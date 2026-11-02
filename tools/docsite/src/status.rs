//! The gap between the documentation (the desired state) and the app, from the pages' status
//! and `Planned` markers. `docsite status` prints it; the site shows it on its Availability page.

use std::fmt::Write as _;

use crate::markdown::Analysis;
use crate::model::{Docs, Status};

pub struct Totals {
    pub implemented: usize,
    pub partial: usize,
    pub planned: usize,
    pub markers: usize,
}

pub fn totals(docs: &Docs, analyses: &[Analysis]) -> Totals {
    let mut totals = Totals {
        implemented: 0,
        partial: 0,
        planned: 0,
        markers: 0,
    };
    for (page, analysis) in docs.pages.iter().zip(analyses) {
        match page.status {
            Status::Implemented => totals.implemented += 1,
            Status::Partial => totals.partial += 1,
            Status::Planned => totals.planned += 1,
        }
        totals.markers += analysis.planned.len();
    }
    totals
}

/// One line per page, then the totals.
pub fn report(docs: &Docs, analyses: &[Analysis]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:<12} {:>7}  PAGE", "STATUS", "PLANNED");
    for (page, analysis) in docs.pages.iter().zip(analyses) {
        let preview = page
            .preview
            .as_deref()
            .map_or(String::new(), |feature| format!("  preview: {feature}"));
        let _ = writeln!(
            out,
            "{:<12} {:>7}  docs/{}{preview}",
            page.status.as_str(),
            analysis.planned.len(),
            page.rel
        );
    }
    let t = totals(docs, analyses);
    let _ = write!(
        out,
        "\n{} pages: {} implemented, {} partial, {} planned; {} Planned markers left\n",
        docs.pages.len(),
        t.implemented,
        t.partial,
        t.planned,
        t.markers
    );
    out
}
