//! JSON-lines output, timing statistics, resident memory.

use std::io::Write;

pub fn emit(v: serde_json::Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// Sorted-copy percentile (nearest rank). `p` in 0..=100.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub p99: f64,
    pub max: f64,
    pub mean: f64,
}

impl Stats {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "n": self.n, "min": self.min, "median": self.median, "p99": self.p99, "max": self.max, "mean": self.mean })
    }
}

pub fn stats(values: &[f64]) -> Stats {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    Stats {
        n: v.len(),
        min: v[0],
        median: percentile(&v, 50.0),
        p99: percentile(&v, 99.0),
        max: *v.last().unwrap(),
        mean,
    }
}

/// VmRSS of this process in KiB, from /proc/self/status.
pub fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}
