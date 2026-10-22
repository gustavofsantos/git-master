//! Counting global allocator. Rhai has no byte limit, so the host measures the
//! bytes live on the calling thread (each run has its own thread) and checks the
//! delta from a progress hook. Counters are thread-local, const-initialised, with
//! no destructor, so they are safe to touch from inside the allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

pub struct Counting;

#[inline]
fn add(delta: isize) {
    LIVE.with(|l| {
        let v = l.get() + delta;
        l.set(v);
        PEAK.with(|p| {
            if v > p.get() {
                p.set(v);
            }
        });
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size() as isize);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            add(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        add(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            add(new_size as isize - layout.size() as isize);
        }
        p
    }
}

/// Net bytes currently attributed to this thread.
pub fn live() -> isize {
    LIVE.with(|l| l.get())
}

/// Reset the peak to the current live value and return it, so a run can read its own peak.
pub fn reset_peak() {
    LIVE.with(|l| PEAK.with(|p| p.set(l.get())));
}

/// Peak live bytes on this thread since the last `reset_peak`.
pub fn peak() -> isize {
    PEAK.with(|p| p.get())
}
