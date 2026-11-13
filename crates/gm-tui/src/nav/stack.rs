//! The screen stack (D241 (1)): opening a screen pushes it; back pops to the one below,
//! as it was left. The root stays. Opening an address already on the stack goes back to
//! it instead of stacking a copy, and the stack is capped.

/// The most screens on the stack, the root included.
#[cfg(test)]
pub const MAX_DEPTH: usize = 32;

/// What a screen is known by: its address, as the trail shows it.
pub trait Addressed {
    fn address(&self) -> String;
}

#[derive(Debug)]
pub struct Stack<S> {
    screens: Vec<S>,
}

impl<S: Addressed> Stack<S> {
    pub fn new(root: S) -> Self {
        Self {
            screens: vec![root],
        }
    }

    pub fn top(&self) -> &S {
        self.screens.last().expect("the root is never popped")
    }

    pub fn top_mut(&mut self) -> &mut S {
        self.screens.last_mut().expect("the root is never popped")
    }

    // Used by the screens pushed on Status (M3 to M6); by tests until then.
    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.screens.len()
    }

    /// Opens `screen`: on top, or, when its address is on the stack already, back to that
    /// one (the screens above it are closed). At the cap, the screen above the root goes.
    // Used by the screens pushed on Status (M3 to M6); by tests until then.
    #[cfg(test)]
    pub fn open(&mut self, screen: S) {
        let address = screen.address();
        if let Some(at) = self.screens.iter().position(|s| s.address() == address) {
            self.screens.truncate(at + 1);
            return;
        }
        if self.screens.len() == MAX_DEPTH {
            self.screens.remove(1);
        }
        self.screens.push(screen);
    }

    /// Back one screen; `false` at the root, where there is nothing to go back to.
    pub fn back(&mut self) -> bool {
        if self.screens.len() == 1 {
            return false;
        }
        self.screens.pop();
        true
    }

    /// Back to the screen at `depth` (0 is the root), as a click on the trail does.
    pub fn back_to(&mut self, depth: usize) {
        self.screens.truncate(depth.max(1).min(self.screens.len()));
    }

    /// The addresses from the root to the top, for the trail.
    pub fn trail(&self) -> Vec<String> {
        self.screens.iter().map(Addressed::address).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Screen(&'static str, usize);

    impl Addressed for Screen {
        fn address(&self) -> String {
            self.0.to_owned()
        }
    }

    #[test]
    fn back_returns_to_the_screen_below_as_it_was() {
        let mut stack = Stack::new(Screen("status", 0));
        stack.top_mut().1 = 7; // the cursor on status
        stack.open(Screen("threads", 0));
        stack.open(Screen("thread 12", 0));
        assert_eq!(stack.trail(), ["status", "threads", "thread 12"]);
        assert!(stack.back());
        assert!(stack.back());
        assert_eq!(stack.top(), &Screen("status", 7));
        assert!(!stack.back(), "the root stays");
    }

    #[test]
    fn opening_an_address_on_the_stack_goes_back_to_it() {
        let mut stack = Stack::new(Screen("status", 0));
        stack.open(Screen("threads", 3));
        stack.open(Screen("thread 12", 0));
        stack.open(Screen("threads", 0));
        assert_eq!(stack.trail(), ["status", "threads"]);
        assert_eq!(stack.top().1, 3, "the one on the stack, not a fresh copy");
        stack.back_to(0);
        assert_eq!(stack.trail(), ["status"]);
    }

    #[test]
    fn the_stack_is_capped_below_the_root() {
        let mut stack = Stack::new(Screen("status", 0));
        let names: Vec<&'static str> = (0..40)
            .map(|n| &*Box::leak(format!("s{n}").into_boxed_str()))
            .collect();
        for name in names {
            stack.open(Screen(name, 0));
        }
        assert_eq!(stack.depth(), MAX_DEPTH);
        assert_eq!(stack.trail()[0], "status");
        assert_eq!(stack.trail().last().map(String::as_str), Some("s39"));
    }
}
