//! The Threads and Thread screens (D241, M3). Threads lists the review threads under who
//! acts next, narrowed by the app's filters (`gm_view::local_review::Filter`). Thread shows
//! one thread: its lines when it was written (Before) and when the agent handled it, or now
//! (After), with what changed marked, and its messages. The texts are read as the app's
//! Review page reads them, through `gm_view::local_review::LocalReview`.

use gm_schema::{CommandKind, Event, OperationId, ReviewComment, ReviewStatus};
use gm_view::local_review::{self, Filter, LocalReview, Version, counts, reads_change};
use gm_view::review::{self, Entry};

use std::collections::HashMap;

use gm_schema::HighlightSpan;

use crate::data::{ColorJob, ColorTarget, LineColors, Repo, split_colors};
use crate::nav::sections::Section;
use crate::status::{Item, thread_groups};

/// The Threads screen's sections for `filter`.
pub fn threads(repo: &Repo, filter: Filter) -> Vec<Section<Item>> {
    let tally = counts(&repo.comments);
    let mut out = vec![Section::leaf(
        "filter",
        Item::Header {
            label: "Filter:",
            text: format!(
                "{} · {} of {} threads · Space f to change",
                filter.label(),
                tally.of(filter),
                repo.comments.len()
            ),
        },
    )];
    let groups = thread_groups(repo, filter);
    if groups.is_empty() {
        let text = if repo.comments.is_empty() {
            "No threads yet. Select lines on Status and press Space c."
        } else {
            "No thread matches this filter."
        };
        out.push(Section::leaf("empty", Item::Note(text.to_owned())));
    }
    out.extend(groups);
    out
}

/// One thread on its screen: the comment it shows and the reads of its two texts.
#[derive(Debug)]
pub struct ThreadPage {
    pub id: i64,
    pub page: LocalReview,
    /// The Before read sent, waiting for its After read, so both are expected together.
    before_read: Option<OperationId>,
    /// The colors of each text, Before then After, with the lines they were taken for.
    colors: [Option<TextColors>; 2],
    /// The text last sent to color, Before then After.
    asked: [String; 2],
}

/// The colors of a text's lines, by index, with the lines they were taken for.
type TextColors = (Vec<String>, HashMap<usize, LineColors>);

/// Where a version's colors are kept.
fn slot(version: Version) -> usize {
    usize::from(version == Version::After)
}

impl ThreadPage {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            page: LocalReview::default(),
            before_read: None,
            colors: [None, None],
            asked: [String::new(), String::new()],
        }
    }

    /// The reads of the thread's texts, as the app's page asks them: one read of its two
    /// snapshots once an agent handled it (D140), else its snapshot and the file now.
    pub fn reads(&mut self, repo: &Repo) -> Vec<CommandKind> {
        let Some(comment) = repo.comment(self.id) else {
            return Vec::new();
        };
        self.page.select(comment);
        let id = self.id;
        if reads_change(comment) {
            vec![CommandKind::LoadReviewChange { id }]
        } else {
            vec![
                CommandKind::LoadReviewSnapshotText { id },
                CommandKind::LoadReviewCurrentText { id },
            ]
        }
    }

    /// The engine took a read of this thread's texts as operation `op`.
    pub fn sent(&mut self, op: OperationId, kind: &CommandKind) {
        match kind {
            CommandKind::LoadReviewChange { id } if *id == self.id => self.page.expect_change(op),
            CommandKind::LoadReviewSnapshotText { id } if *id == self.id => {
                self.before_read = Some(op);
            }
            CommandKind::LoadReviewCurrentText { id } if *id == self.id => {
                if let Some(before) = self.before_read.take() {
                    self.page.expect(before, op);
                }
            }
            _ => {}
        }
    }

    /// Takes an answer to one of its reads; false when it is not one.
    pub fn received(&mut self, event: &Event) -> bool {
        match event {
            Event::ReviewChange {
                id,
                comment,
                change,
                ..
            } => self.page.received_change(*id, *comment, change.clone()),
            Event::ReviewSnapshotText {
                id, comment, lines, ..
            } => self.page.received_before(*id, *comment, lines.clone()),
            Event::ReviewCurrentText {
                id, comment, lines, ..
            } => self.page.received_after(*id, *comment, lines.clone()),
            _ => false,
        }
    }

    /// The text of `version` to color, as the file at `path` reads.
    fn color_job(&self, path: &str, version: Version) -> Option<ColorJob> {
        let target = ColorTarget::Thread {
            id: self.id,
            after: version == Version::After,
        };
        let mut job = ColorJob::new(target, path);
        for (index, line) in self.page.lines(version)?.iter().enumerate() {
            job.push(Some((index, 0)), line);
        }
        job.finish()
    }

    /// The texts to color that were not sent yet (D251 (1)).
    pub fn color_jobs(&mut self, repo: &Repo) -> Vec<ColorJob> {
        let Some(comment) = repo.comment(self.id) else {
            return Vec::new();
        };
        let mut jobs = Vec::new();
        for version in [Version::Before, Version::After] {
            if let Some(job) = self.color_job(&comment.path, version)
                && self.asked[slot(version)] != job.text
            {
                self.asked[slot(version)].clone_from(&job.text);
                jobs.push(job);
            }
        }
        jobs
    }

    /// Takes the colors of `job`, when its text is still the one shown.
    pub fn colored(&mut self, repo: &Repo, job: &ColorJob, spans: &[HighlightSpan]) {
        let ColorTarget::Thread { after, .. } = job.target else {
            return;
        };
        let version = if after {
            Version::After
        } else {
            Version::Before
        };
        let Some(comment) = repo.comment(self.id) else {
            return;
        };
        if self.color_job(&comment.path, version).as_ref() != Some(job) {
            return;
        }
        let lines = self.page.lines(version).unwrap_or_default().to_vec();
        let colors = split_colors(job, spans)
            .into_iter()
            .map(|((index, _), colors)| (index, colors))
            .collect();
        self.colors[slot(version)] = Some((lines, colors));
    }

    /// Lines added and removed between the two texts, once both are read.
    fn measured(&self) -> Option<(usize, usize)> {
        self.page.lines(Version::Before)?;
        self.page.lines(Version::After)?;
        let count = |version| self.page.marks(version).iter().filter(|m| **m).count();
        Some((count(Version::After), count(Version::Before)))
    }

    /// The Thread screen's sections.
    pub fn sections(&self, repo: &Repo) -> Vec<Section<Item>> {
        let Some(comment) = repo.comment(self.id) else {
            return vec![Section::leaf(
                "gone",
                Item::Note(format!("Thread {} was deleted.", self.id)),
            )];
        };
        let (start, end) = review::shown_range(comment);
        let status = match comment.status {
            ReviewStatus::Open => "Open",
            ReviewStatus::Handled => "Handled",
            ReviewStatus::Resolved => "Resolved",
        };
        let mut out = vec![Section::leaf(
            "head",
            Item::Header {
                label: "Thread:",
                text: format!(
                    "{} · {} {} · {status}",
                    self.id,
                    comment.path,
                    review::range_label(start, end)
                ),
            },
        )];
        if reads_change(comment)
            && let Some((added, removed)) = self.measured()
        {
            out.push(Section::leaf(
                "measured",
                Item::Note(format!(
                    "The agent changed +{added} -{removed} in these lines, as GitMaster measured it."
                )),
            ));
        }
        for version in [Version::Before, Version::After] {
            out.push(self.text_section(comment, version));
        }
        out.push(messages(comment));
        out
    }

    fn text_section(&self, comment: &ReviewComment, version: Version) -> Section<Item> {
        let name = version.label();
        let children = match self.page.lines(version) {
            None => vec![Section::leaf(
                format!("{name}/reading"),
                Item::Note("Reading…".to_owned()),
            )],
            Some([]) => vec![Section::leaf(
                format!("{name}/empty"),
                Item::Note(local_review::empty_text(version, comment).to_owned()),
            )],
            Some(lines) => {
                let first = self.page.first_line(comment, version);
                let marks = self.page.marks(version);
                // Colors taken for other lines would sit on the wrong text.
                let colors = self.colors[slot(version)]
                    .as_ref()
                    .filter(|(of, _)| of.as_slice() == lines)
                    .map(|(_, colors)| colors);
                lines
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        let number = first + u32::try_from(index).unwrap_or(u32::MAX);
                        Section::leaf(
                            format!("{name}/{index}"),
                            Item::Text {
                                version,
                                number,
                                text: text.clone(),
                                marked: marks.get(index).copied().unwrap_or(false),
                                colors: colors
                                    .and_then(|colors| colors.get(&index))
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        )
                    })
                    .collect()
            }
        };
        Section::with(
            format!("version/{name}"),
            Item::Version {
                version,
                caption: local_review::caption(version, comment),
            },
            children,
        )
    }
}

/// The comment and every reply, as the cards under a diff show them, all opened.
fn messages(comment: &ReviewComment) -> Section<Item> {
    let children: Vec<Section<Item>> = review::card_entries(comment, true, true)
        .into_iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            Entry::Card { id, part } => Some(Section::leaf(
                format!("message/{index}"),
                Item::Card { id, part },
            )),
            _ => None,
        })
        .collect();
    Section::with(
        "messages",
        Item::Messages {
            count: 1 + comment.replies.len(),
        },
        children,
    )
}

/// The filter an action names.
pub fn filter_of(action: &str) -> Option<Filter> {
    Some(match action {
        "filter.open" => Filter::Open,
        "filter.resolved" => Filter::Resolved,
        "filter.outdated" => Filter::Outdated,
        "filter.needs_answer" => Filter::NeedsAnswer,
        "filter.agent" => Filter::Agent,
        "filter.all" => Filter::All,
        _ => return None,
    })
}
