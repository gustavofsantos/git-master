//! The Agent thread screen (D241, M6): a captured agent session read as the app reads it
//! (`gm_view::thread_turns`), by sections: the **Request**, the **Decisions** (the questions
//! the agent asked and the answers), the **Changes** to each file with their patches, the
//! **Runs** of commands and how they ended, the **Files read** and the **Timeline** of turns.
//! Text from a thread is untrusted: it is shown as plain text, one line per row, capped.

use std::collections::{HashMap, HashSet};

use gm_schema::{CommandKind, HighlightSpan, ThreadDetail, ThreadEdit};
use gm_view::thread_turns::{
    self, Block, Call, CallStatus, PatchKind, duration_label, file_lines, patch_lines,
};

use crate::data::{ColorJob, ColorTarget, LineColors, split_colors};
use crate::nav::sections::Section;
use crate::status::Item;

/// Entries asked per read; the screen follows the pages up to `MOST_ENTRIES`.
pub const PAGE: u32 = 500;
/// The most entries the screen reads of one thread.
const MOST_ENTRIES: usize = 5_000;
/// The most lines of a result or a patch a row group shows.
const MOST_LINES: usize = 40;
/// The most characters of thread text one row shows.
const TEXT_CAP: usize = 200;

/// A row of the Agent thread screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRow {
    /// The title of a section, with how many rows it holds.
    Heading { title: &'static str, count: usize },
    /// A line of text: the request, an answer, a result, a message.
    Text { text: String, quiet: bool },
    /// A tool call: its tool, its argument, how it ended and how long it took.
    Call {
        tool: String,
        summary: String,
        status: CallStatus,
        duration: Option<String>,
    },
    /// A file the agent changed, with the lines all its edits added and removed.
    File {
        path: String,
        added: u32,
        removed: u32,
    },
    /// A line of an edit's patch, with the colors of its code: ranges of `text` after its
    /// first character (the `+`, `-` or space).
    Patch {
        kind: PatchKind,
        text: String,
        colors: LineColors,
    },
    /// A turn of the timeline: its number and the first line of its prompt.
    Turn { number: u32, prompt: String },
}

impl AgentRow {
    /// The words `/` searches.
    pub fn text(&self) -> String {
        match self {
            AgentRow::Heading { title, .. } => (*title).to_owned(),
            AgentRow::Text { text, .. } | AgentRow::Patch { text, .. } => text.clone(),
            AgentRow::Call { tool, summary, .. } => format!("{tool} {summary}"),
            AgentRow::File { path, .. } => path.clone(),
            AgentRow::Turn { prompt, .. } => prompt.clone(),
        }
    }

    /// The file a row is about, for `g f` and `g b`.
    pub fn path(&self) -> Option<&str> {
        match self {
            AgentRow::File { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// One thread on its screen and the reads of its pages.
#[derive(Debug)]
pub struct AgentPage {
    pub uid: String,
    pub detail: Option<ThreadDetail>,
    pub failure: Option<String>,
    /// The colors of patch lines, by edit and line index in its patch (D251 (5)).
    colors: HashMap<(i64, usize), LineColors>,
    /// The edits sent to color.
    asked: HashSet<i64>,
}

impl AgentPage {
    pub fn new(uid: String) -> Self {
        Self {
            uid,
            detail: None,
            failure: None,
            colors: HashMap::new(),
            asked: HashSet::new(),
        }
    }

    /// The patches to color that were not sent yet.
    pub fn color_jobs(&mut self) -> Vec<ColorJob> {
        let Some(detail) = &self.detail else {
            return Vec::new();
        };
        detail
            .edits
            .iter()
            .filter(|edit| self.asked.insert(edit.id))
            .flat_map(patch_color_jobs)
            .collect()
    }

    /// Takes the colors of `job`, a side of one of its edits.
    pub fn colored(&mut self, job: &ColorJob, spans: &[HighlightSpan]) {
        let ColorTarget::Patch { edit, .. } = job.target else {
            return;
        };
        let current = self
            .detail
            .iter()
            .flat_map(|detail| &detail.edits)
            .find(|known| known.id == edit)
            .is_some_and(|known| patch_color_jobs(known).contains(job));
        if current {
            for ((index, _), colors) in split_colors(job, spans) {
                self.colors.insert((edit, index), colors);
            }
        }
    }

    /// The first read of the thread.
    pub fn first_read(&self) -> CommandKind {
        CommandKind::ReadThread {
            uid: self.uid.clone(),
            from_seq: 0,
            limit: PAGE,
        }
    }

    /// Takes a page of the thread, and returns the read of the next one when there is more
    /// and the screen has room for it.
    pub fn received(&mut self, page: &ThreadDetail) -> Option<CommandKind> {
        let detail = self.detail.get_or_insert_with(|| ThreadDetail {
            entries: Vec::new(),
            edits: Vec::new(),
            ..page.clone()
        });
        for entry in &page.entries {
            if !detail.entries.iter().any(|known| known.id == entry.id) {
                detail.entries.push(entry.clone());
            }
        }
        for edit in &page.edits {
            if !detail.edits.iter().any(|known| known.id == edit.id) {
                detail.edits.push(edit.clone());
            }
        }
        detail.summary = page.summary.clone();
        let last = page.entries.iter().map(|entry| entry.seq).max()?;
        (page.has_more && detail.entries.len() < MOST_ENTRIES).then(|| CommandKind::ReadThread {
            uid: self.uid.clone(),
            from_seq: last + 1,
            limit: PAGE,
        })
    }

    /// The screen's sections.
    pub fn sections(&self) -> Vec<Section<Item>> {
        let Some(detail) = &self.detail else {
            let text = self
                .failure
                .clone()
                .unwrap_or_else(|| "Reading the thread…".to_owned());
            return vec![Section::leaf("note", Item::Note(text))];
        };
        let summary = &detail.summary;
        let mut out = vec![
            Section::leaf(
                "title",
                Item::Header {
                    label: "Thread:",
                    text: one_line(&summary.title),
                },
            ),
            Section::leaf(
                "agent",
                Item::Header {
                    label: "Agent:",
                    text: format!(
                        "{} · {} · {} · {} · {}",
                        summary.harness,
                        summary.model.as_deref().unwrap_or("model unknown"),
                        summary.branch.as_deref().unwrap_or("detached HEAD"),
                        counted(summary.entry_count, "entry", "entries"),
                        counted(summary.edit_count, "edit", "edits")
                    ),
                },
            ),
        ];
        let turns = thread_turns::turns(detail);
        let calls: Vec<&Call> = turns
            .iter()
            .flat_map(|turn| &turn.blocks)
            .flat_map(calls_in)
            .collect();
        out.push(request(&turns));
        out.push(decisions(&calls));
        out.push(changes(detail, &self.colors));
        out.push(runs(&calls));
        out.push(files_read(&calls, &summary.worktree));
        out.push(timeline(&turns));
        if detail.entries.len() >= MOST_ENTRIES {
            out.push(Section::leaf(
                "more",
                Item::Note(format!("Only the first {MOST_ENTRIES} entries are read.")),
            ));
        }
        out
    }
}

/// Every call of a block, its sub-agent's calls included, in order.
fn calls_in(block: &Block) -> Vec<&Call> {
    match block {
        Block::Message(_) => Vec::new(),
        Block::Call(call) => std::iter::once(call)
            .chain(call.children.iter().flat_map(calls_in))
            .collect(),
    }
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TEXT_CAP {
        return flat;
    }
    let cut: String = flat.chars().take(TEXT_CAP - 1).collect();
    format!("{cut}…")
}

fn row(id: String, row: AgentRow) -> Section<Item> {
    Section::leaf(id, Item::Agent(row))
}

fn counted(count: u32, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A section's heading over its rows; an empty one says so on a row of its own.
fn heading(id: &str, title: &'static str, mut children: Vec<Section<Item>>) -> Section<Item> {
    let count = children.len();
    if children.is_empty() {
        let text = match title {
            "Decisions" => "The agent asked you nothing.",
            "Changes" => "The agent changed no file.",
            "Runs" => "The agent ran no command.",
            "Files read" => "The agent read no file.",
            _ => "Nothing.",
        };
        children.push(row(
            format!("{id}/none"),
            AgentRow::Text {
                text: text.to_owned(),
                quiet: true,
            },
        ));
    }
    Section::with(
        id,
        Item::Agent(AgentRow::Heading { title, count }),
        children,
    )
}

/// Lines of `text` as rows, at most `MOST_LINES`, then how many were left out.
fn text_rows(id: &str, text: &str, quiet: bool) -> Vec<Section<Item>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<Section<Item>> = lines
        .iter()
        .take(MOST_LINES)
        .enumerate()
        .map(|(index, line)| {
            row(
                format!("{id}/{index}"),
                AgentRow::Text {
                    text: one_line(line),
                    quiet,
                },
            )
        })
        .collect();
    if lines.len() > MOST_LINES {
        out.push(row(
            format!("{id}/more"),
            AgentRow::Text {
                text: format!("… {} more lines", lines.len() - MOST_LINES),
                quiet: true,
            },
        ));
    }
    out
}

fn call_row(id: String, call: &Call, children: Vec<Section<Item>>) -> Section<Item> {
    Section::with(
        id,
        Item::Agent(AgentRow::Call {
            tool: call.tool.clone(),
            summary: call.summary.clone(),
            status: call.status,
            duration: call.duration_ms.map(duration_label),
        }),
        children,
    )
}

fn request(turns: &[thread_turns::Turn]) -> Section<Item> {
    let prompt = turns
        .iter()
        .find_map(|turn| turn.prompt.as_deref())
        .unwrap_or("");
    heading("request", "Request", text_rows("request", prompt, false))
}

/// A call that asks the person something: Claude Code's `AskUserQuestion`, and the like.
fn asks(call: &Call) -> bool {
    let tool = call.tool.to_ascii_lowercase();
    tool.contains("askuser") || tool.contains("ask_user") || tool.contains("question")
}

fn decisions(calls: &[&Call]) -> Section<Item> {
    let children = calls
        .iter()
        .filter(|call| asks(call))
        .map(|call| {
            let id = format!("decision/{}", call.entry_id);
            let answer = call
                .result
                .as_ref()
                .map_or("No answer was recorded.", |result| result.text.as_str());
            call_row(id.clone(), call, text_rows(&id, answer, false))
        })
        .collect();
    heading("decisions", "Decisions", children)
}

fn changes(detail: &ThreadDetail, colors: &HashMap<(i64, usize), LineColors>) -> Section<Item> {
    let children = file_lines(detail)
        .into_iter()
        .filter(|file| file.edited)
        .map(|file| {
            let id = format!("change/{}", file.path);
            let patch: Vec<Section<Item>> = detail
                .edits
                .iter()
                .filter(|edit| edit.path == file.path)
                .flat_map(|edit| {
                    let lines = patch_lines(&edit.patch);
                    let mut rows: Vec<Section<Item>> = lines
                        .iter()
                        .take(MOST_LINES)
                        .enumerate()
                        .map(|(index, (kind, text))| {
                            row(
                                format!("{id}/{}/{index}", edit.id),
                                AgentRow::Patch {
                                    kind: *kind,
                                    text: (*text).to_owned(),
                                    colors: colors
                                        .get(&(edit.id, index))
                                        .cloned()
                                        .unwrap_or_default(),
                                },
                            )
                        })
                        .collect();
                    if lines.len() > MOST_LINES {
                        rows.push(row(
                            format!("{id}/{}/more", edit.id),
                            AgentRow::Text {
                                text: format!("… {} more lines", lines.len() - MOST_LINES),
                                quiet: true,
                            },
                        ));
                    }
                    rows
                })
                .collect();
            Section::with(
                id,
                Item::Agent(AgentRow::File {
                    path: file.path,
                    added: file.added,
                    removed: file.removed,
                }),
                patch,
            )
            .folded()
        })
        .collect();
    heading("changes", "Changes", children)
}

/// The two texts to color of an edit's patch (D251 (5)): the old side (context and removed
/// lines) and the new (context and added lines), each line without its first character.
/// Header lines are left out; context is colored from the new side.
fn patch_color_jobs(edit: &ThreadEdit) -> Vec<ColorJob> {
    let lines = patch_lines(&edit.patch);
    [false, true]
        .into_iter()
        .filter_map(|old| {
            let target = ColorTarget::Patch { edit: edit.id, old };
            let mut job = ColorJob::new(target, &edit.path);
            for (index, (kind, text)) in lines.iter().enumerate() {
                let key = Some((index, 0));
                let code = text.get(1..).unwrap_or_default();
                match (kind, old) {
                    (PatchKind::Context, _) if text.is_empty() => {
                        job.push(key.filter(|_| !old), "");
                    }
                    (PatchKind::Context, _) if text.starts_with(' ') => {
                        job.push(key.filter(|_| !old), code);
                    }
                    (PatchKind::Added, false) | (PatchKind::Removed, true) => job.push(key, code),
                    _ => {}
                }
            }
            job.finish()
        })
        .collect()
}

/// A call that runs a command: Claude Code's `Bash`, and shells by other names.
fn runs_command(call: &Call) -> bool {
    let tool = call.tool.to_ascii_lowercase();
    tool == "bash" || tool.contains("shell") || tool.contains("exec") || tool.contains("command")
}

fn runs(calls: &[&Call]) -> Section<Item> {
    let children = calls
        .iter()
        .filter(|call| runs_command(call))
        .map(|call| {
            let id = format!("run/{}", call.entry_id);
            let output = call
                .result
                .as_ref()
                .map_or("", |result| result.text.as_str());
            call_row(id.clone(), call, text_rows(&id, output, true)).folded()
        })
        .collect();
    heading("runs", "Runs", children)
}

/// A call that reads a file: Claude Code's `Read`, and readers by other names.
fn reads_file(call: &Call) -> bool {
    let tool = call.tool.to_ascii_lowercase();
    tool == "read" || tool.contains("read_file") || tool == "view"
}

fn files_read(calls: &[&Call], worktree: &str) -> Section<Item> {
    let mut paths: Vec<&str> = Vec::new();
    for call in calls.iter().filter(|call| reads_file(call)) {
        // A path inside the worktree shows relative to it, as the rest of the screen does.
        let path = call
            .summary
            .strip_prefix(worktree)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(&call.summary);
        if !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
    }
    let children = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            row(
                format!("read/{index}"),
                AgentRow::Text {
                    text: (*path).to_owned(),
                    quiet: false,
                },
            )
        })
        .collect();
    heading("read", "Files read", children)
}

fn timeline(turns: &[thread_turns::Turn]) -> Section<Item> {
    let children = turns
        .iter()
        .map(|turn| {
            let id = format!("turn/{}", turn.number);
            let blocks = turn
                .blocks
                .iter()
                .enumerate()
                .map(|(index, block)| match block {
                    Block::Message(message) => row(
                        format!("{id}/{index}"),
                        AgentRow::Text {
                            text: one_line(message.text.lines().next().unwrap_or("")),
                            quiet: true,
                        },
                    ),
                    Block::Call(call) => call_row(format!("{id}/{index}"), call, Vec::new()),
                })
                .collect();
            Section::with(
                id,
                Item::Agent(AgentRow::Turn {
                    number: turn.number,
                    prompt: one_line(
                        turn.prompt
                            .as_deref()
                            .unwrap_or("")
                            .lines()
                            .next()
                            .unwrap_or(""),
                    ),
                }),
                blocks,
            )
            .folded()
        })
        .collect();
    heading("timeline", "Timeline", children).folded()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gm_schema::{EditChange, EntryKind, ThreadEdit, ThreadEntry, ThreadSummary};

    fn entry(
        id: i64,
        kind: EntryKind,
        source: &str,
        tool: Option<&str>,
        text: &str,
    ) -> ThreadEntry {
        ThreadEntry {
            id,
            seq: u32::try_from(id).unwrap(),
            turn: 1,
            kind,
            source_id: source.to_owned(),
            parent_id: None,
            tool: tool.map(str::to_owned),
            text: text.to_owned(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            at: id * 1_000,
        }
    }

    fn call(id: i64, source: &str, tool: &str, args: &str) -> ThreadEntry {
        ThreadEntry {
            args_json: Some(args.to_owned()),
            ..entry(id, EntryKind::ToolCall, source, Some(tool), "")
        }
    }

    fn result(id: i64, source: &str, text: &str, error: bool) -> ThreadEntry {
        ThreadEntry {
            is_error: error,
            ..entry(
                id,
                EntryKind::ToolResult,
                &format!("{source}:result"),
                None,
                text,
            )
        }
    }

    pub fn detail() -> ThreadDetail {
        ThreadDetail {
            summary: ThreadSummary {
                uid: "t1".to_owned(),
                harness: "claude-code".to_owned(),
                harness_version: None,
                session_id: "s".to_owned(),
                worktree: "/repo".to_owned(),
                branch: Some("feature".to_owned()),
                model: Some("a-model".to_owned()),
                title: "Add the parser".to_owned(),
                note: String::new(),
                starred: false,
                tags: Vec::new(),
                started_at: 0,
                last_event_at: 0,
                ended_at: None,
                entry_count: 9,
                edit_count: 1,
                redaction_count: 0,
                workspace: None,
            },
            parent: None,
            entries: vec![
                entry(
                    1,
                    EntryKind::User,
                    "u1",
                    None,
                    "Add a parser for the ledger.\nKeep it small.",
                ),
                call(
                    2,
                    "c1",
                    "AskUserQuestion",
                    r#"{"question":"Strict or lenient?"}"#,
                ),
                result(3, "c1", "Strict.", false),
                call(4, "c2", "Read", r#"{"file_path":"src/ledger.rs"}"#),
                result(5, "c2", "fn main() {}", false),
                call(6, "c3", "Edit", r#"{"file_path":"src/parser.rs"}"#),
                result(7, "c3", "ok", false),
                call(8, "c4", "Bash", r#"{"command":"cargo test"}"#),
                result(9, "c4", "1 failed", true),
            ],
            edits: vec![ThreadEdit {
                id: 1,
                entry_id: 6,
                path: "src/parser.rs".to_owned(),
                old_path: None,
                change: EditChange::Create,
                patch: "@@ -0,0 +1 @@\n+pub fn parse() {}".to_owned(),
                added: 1,
                removed: 0,
                pre_oid: None,
                post_oid: None,
                from_input: false,
                at: 6_000,
                member: None,
            }],
            links: Vec::new(),
            relations: Vec::new(),
            redactions: Vec::new(),
            has_more: false,
        }
    }

    fn texts(sections: &[Section<Item>], depth: usize, out: &mut Vec<String>) {
        for section in sections {
            out.push(format!("{}{:?}", "  ".repeat(depth), section.item));
            texts(&section.children, depth + 1, out);
        }
    }

    #[test]
    fn a_thread_reads_as_request_decisions_changes_runs_files_and_timeline() {
        let mut page = AgentPage::new("t1".to_owned());
        assert_eq!(page.received(&detail()), None, "one page");
        let mut rows = Vec::new();
        texts(&page.sections(), 0, &mut rows);
        let find = |needle: &str| rows.iter().any(|r| r.contains(needle));
        assert!(
            find("Heading { title: \"Request\", count: 2 }"),
            "{rows:#?}"
        );
        assert!(find("text: \"Add a parser for the ledger.\""), "{rows:#?}");
        assert!(
            find("Heading { title: \"Decisions\", count: 1 }"),
            "{rows:#?}"
        );
        assert!(find("summary: \"Strict or lenient?\""), "{rows:#?}");
        assert!(find("text: \"Strict.\""), "{rows:#?}");
        assert!(
            find("File { path: \"src/parser.rs\", added: 1, removed: 0 }"),
            "{rows:#?}"
        );
        assert!(
            find("Patch { kind: Added, text: \"+pub fn parse() {}\""),
            "{rows:#?}"
        );
        assert!(
            find("tool: \"Bash\", summary: \"cargo test\", status: Failed"),
            "{rows:#?}"
        );
        assert!(
            find("Heading { title: \"Files read\", count: 1 }"),
            "{rows:#?}"
        );
        assert!(find("text: \"src/ledger.rs\""), "{rows:#?}");
        assert!(
            find("Turn { number: 1, prompt: \"Add a parser for the ledger.\" }"),
            "{rows:#?}"
        );
    }

    #[test]
    fn patches_are_colored_per_side_without_their_signs_and_headers() {
        let mut edit = detail().edits.remove(0);
        edit.patch = "--- a/src/parser.rs\n+++ b/src/parser.rs\n@@ -1,2 +1,2 @@\n fn a() {\n-    old();\n+    new();".to_owned();
        let jobs = patch_color_jobs(&edit);
        let [new, old] = jobs.as_slice() else {
            panic!("two sides: {jobs:?}");
        };
        assert_eq!(new.text, "fn a() {\n    new();\n");
        assert_eq!(new.lines, [(Some((3, 0)), 0), (Some((5, 0)), 9)]);
        // The old side parses its context but colors only the removed line.
        assert_eq!(old.text, "fn a() {\n    old();\n");
        assert_eq!(old.lines, [(None, 0), (Some((4, 0)), 9)]);
    }

    #[test]
    fn a_patch_line_takes_its_colors_once() {
        let mut page = AgentPage::new("t1".to_owned());
        page.received(&detail());
        let jobs = page.color_jobs();
        assert_eq!(jobs.len(), 1, "the patch only adds: {jobs:?}");
        let stop = std::sync::atomic::AtomicBool::new(false);
        let language = gm_core::syntax::language_for_path(&jobs[0].path).unwrap();
        let spans = gm_core::syntax::highlight(language, &jobs[0].text, &stop).unwrap();
        page.colored(&jobs[0], &spans);
        let mut rows = Vec::new();
        texts(&page.sections(), 0, &mut rows);
        let keyword = format!("colors: [(0, 3, {:?})", gm_schema::SyntaxRole::Keyword);
        assert!(
            rows.iter()
                .any(|row| row.contains("Patch") && row.contains(&keyword)),
            "`pub` is a keyword, counted after the sign: {rows:#?}"
        );
        page.received(&detail());
        assert!(page.color_jobs().is_empty(), "an edit is colored once");
    }

    #[test]
    fn a_long_thread_is_read_page_after_page() {
        let mut page = AgentPage::new("t1".to_owned());
        let mut first = detail();
        first.has_more = true;
        first.entries.truncate(4);
        assert_eq!(
            page.received(&first),
            Some(CommandKind::ReadThread {
                uid: "t1".to_owned(),
                from_seq: 5,
                limit: PAGE
            })
        );
        assert_eq!(page.received(&detail()), None);
        assert_eq!(
            page.detail.as_ref().unwrap().entries.len(),
            9,
            "no entry twice"
        );
    }
}
