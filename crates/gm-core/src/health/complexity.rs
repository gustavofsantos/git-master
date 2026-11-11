//! Code health: how deeply the code of a file is nested, read from its indentation (D229,
//! docs/guide/code-health.md, "The indicators"). `measure` is a pure function of the bytes of a
//! file. `read` measures the blobs of the hot files at their commit, through one
//! `git cat-file --batch` process, and gives each its percentile among the measured files.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use gm_schema::Complexity;

use super::ingest::git;

/// The hottest files whose complexity is measured (D229 (1)).
pub const MEASURED_FILES: usize = 1_000;
/// The largest file measured. A larger file, or one that is not text, has no complexity.
pub const MAX_BYTES: usize = 1024 * 1024;
/// The bytes searched for a NUL to tell a binary file from a text one.
const SNIFF_BYTES: usize = 8 * 1024;
/// Columns per level of nesting: a tab, or four spaces.
const LEVEL_COLUMNS: u32 = 4;

/// The measure of one file: its non-blank lines, the deepest nesting, the mean nesting and the
/// total nesting (`Complexity`).
pub type Measure = (u32, u32, f64, u32);

/// Measures the indentation of `text`. `None` for a file with a NUL byte in its first 8 KiB, or
/// one larger than 1 MiB. Blank lines (only whitespace) do not count; a trailing `\r` is not
/// content; a tab moves to the next multiple of four columns, a space takes one column.
#[must_use]
pub fn measure(text: &[u8]) -> Option<Measure> {
    if text.len() > MAX_BYTES || text[..text.len().min(SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let mut lines = 0_u32;
    let mut deepest = 0_u32;
    let mut columns = 0_u32;
    for line in text.split(|&byte| byte == b'\n') {
        let line = match line {
            [rest @ .., b'\r'] => rest,
            _ => line,
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let width = indent_columns(line);
        lines += 1;
        deepest = deepest.max(width);
        columns += width;
    }
    let mean = if lines == 0 {
        0.0
    } else {
        round(
            f64::from(columns) / f64::from(LEVEL_COLUMNS) / f64::from(lines),
            2,
        )
    };
    // The sum of the levels, rounded to the nearest (halves up).
    let total = (columns + LEVEL_COLUMNS / 2) / LEVEL_COLUMNS;
    Some((lines, deepest / LEVEL_COLUMNS, mean, total))
}

/// The columns of a line's leading whitespace.
fn indent_columns(line: &[u8]) -> u32 {
    let mut columns = 0_u32;
    for &byte in line {
        match byte {
            b' ' => columns += 1,
            b'\t' => columns = (columns / LEVEL_COLUMNS + 1) * LEVEL_COLUMNS,
            _ => break,
        }
    }
    columns
}

/// The share of `sorted` (the totals of the measured files, in order) that is lower than `value`,
/// 0 to 100 with one decimal.
#[must_use]
pub fn percentile(sorted: &[u32], value: u32) -> f64 {
    let all = u32::try_from(sorted.len()).unwrap_or(u32::MAX);
    if all == 0 {
        return 0.0;
    }
    let lower = u32::try_from(sorted.partition_point(|&other| other < value)).unwrap_or(u32::MAX);
    round(100.0 * f64::from(lower) / f64::from(all), 1)
}

/// The complexity of each file of `ids` (its blob id, or `None` for a file that is not measured),
/// at the commit. Each measured file is given its percentile among the measured files by total
/// nesting. One `git cat-file --batch` process; `cancel` ends it with `Err("cancelled")`.
pub fn read(
    root: &Path,
    ids: &[Option<String>],
    cancel: &AtomicBool,
) -> Result<Vec<Option<Complexity>>, String> {
    let slots: Vec<usize> = ids
        .iter()
        .enumerate()
        .filter_map(|(slot, id)| id.as_ref().map(|_| slot))
        .collect();
    let requests: Vec<String> = ids.iter().flatten().cloned().collect();
    let measures = measure_blobs(root, &requests, cancel)?;
    let mut totals: Vec<u32> = measures.iter().flatten().map(|measure| measure.3).collect();
    totals.sort_unstable();
    let mut out = vec![None; ids.len()];
    for (slot, measure) in slots.into_iter().zip(measures) {
        if let Some((lines, indent_max, indent_mean, indent_total)) = measure {
            out[slot] = Some(Complexity {
                lines,
                indent_max,
                indent_mean,
                indent_total,
                percentile: percentile(&totals, indent_total),
            });
        }
    }
    Ok(out)
}

/// The measure of each blob of `ids`, in order; `None` for one git does not have. The writer
/// runs on a thread of its own, so a large request cannot block the reading.
fn measure_blobs(
    root: &Path,
    ids: &[String],
    cancel: &AtomicBool,
) -> Result<Vec<Option<Measure>>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("git has no pipes".to_owned());
    };
    let requests = ids.to_vec();
    let writer = std::thread::spawn(move || {
        for id in requests {
            if writeln!(stdin, "{id}").is_err() {
                return;
            }
        }
    });
    let answers = read_answers(stdout, ids.len(), cancel);
    if answers.is_err() {
        let _ = child.kill();
    }
    let _ = writer.join();
    let _ = child.wait();
    answers
}

/// Reads `count` answers of `cat-file --batch`: `<id> blob <size>` then the content and a
/// newline, or `<id> missing`.
fn read_answers(
    stdout: ChildStdout,
    count: usize,
    cancel: &AtomicBool,
) -> Result<Vec<Option<Measure>>, String> {
    let mut reader = BufReader::new(stdout);
    let mut out = Vec::with_capacity(count);
    let mut header = String::new();
    for _ in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_owned());
        }
        header.clear();
        let read = reader
            .read_line(&mut header)
            .map_err(|e| format!("cannot read git's output: {e}"))?;
        if read == 0 {
            return Err("git cat-file ended before its answers".to_owned());
        }
        let line = header.trim_end();
        if line.ends_with(" missing") {
            out.push(None);
            continue;
        }
        let mut fields = line.split(' ');
        let (Some(_id), Some("blob"), Some(size)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("git cat-file: bad answer {line:?}"));
        };
        let size = size
            .parse::<usize>()
            .map_err(|_| format!("git cat-file: bad size in {line:?}"))?;
        if size > MAX_BYTES {
            return Err(format!(
                "git cat-file: {size} bytes is over the measured size"
            ));
        }
        let mut content = vec![0_u8; size];
        reader
            .read_exact(&mut content)
            .map_err(|e| format!("cannot read git's output: {e}"))?;
        let mut newline = [0_u8; 1];
        reader
            .read_exact(&mut newline)
            .map_err(|e| format!("cannot read git's output: {e}"))?;
        out.push(measure(&content));
    }
    Ok(out)
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_are_one_level_each() {
        // Depths 0, 1, 2 and 1: max 2, mean 1.0, total 4.
        let text = b"x\n\ty\n\t\tz\n\tw\n";
        assert_eq!(measure(text), Some((4, 2, 1.0, 4)));
    }

    #[test]
    fn four_spaces_are_one_level_and_a_tab_is_four_columns() {
        assert_eq!(measure(b"    a\n        b\n"), Some((2, 2, 1.5, 3)));
        // Three spaces then a tab reach column 4: one level.
        assert_eq!(measure(b"   \tx\n"), Some((1, 1, 1.0, 1)));
    }

    #[test]
    fn two_spaces_are_half_a_level() {
        // Columns 2 and 4: levels 0.5 and 1. The total is 1.5, rounded up to 2; the mean is 0.75.
        assert_eq!(measure(b"  a\n    b\n"), Some((2, 1, 0.75, 2)));
    }

    #[test]
    fn blank_lines_do_not_count() {
        assert_eq!(measure(b"a\n\n   \n\tb\n"), Some((2, 1, 0.5, 1)));
        assert_eq!(measure(b"\n\n"), Some((0, 0, 0.0, 0)));
        assert_eq!(measure(b""), Some((0, 0, 0.0, 0)));
    }

    #[test]
    fn crlf_is_the_same_as_lf() {
        assert_eq!(
            measure(b"\tif\r\n\t\tx\r\n\r\n"),
            measure(b"\tif\n\t\tx\n\n")
        );
        assert_eq!(measure(b"\tif\r\n\t\tx\r\n"), Some((2, 2, 1.5, 3)));
    }

    #[test]
    fn binary_text_has_no_measure_but_a_nul_after_the_sniff_does_not_count() {
        assert_eq!(measure(b"abc\0def\n"), None);
        let mut late = vec![b'a'; SNIFF_BYTES];
        late.push(0);
        assert!(measure(&late).is_some());
    }

    #[test]
    fn a_file_over_one_mebibyte_has_no_measure() {
        let exact = vec![b'a'; MAX_BYTES];
        assert!(measure(&exact).is_some());
        let over = vec![b'a'; MAX_BYTES + 1];
        assert_eq!(measure(&over), None);
    }

    #[test]
    fn a_percentile_is_the_share_of_lower_totals() {
        assert_eq!(percentile(&[1, 2, 3, 4], 3), 50.0);
        assert_eq!(percentile(&[1, 2, 3, 4], 1), 0.0);
        assert_eq!(percentile(&[5], 5), 0.0);
        assert_eq!(percentile(&[1, 2, 3], 2), 33.3);
        assert_eq!(percentile(&[], 9), 0.0);
    }
}
