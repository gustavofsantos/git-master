//! The rows of the thread shown (T5, the detail list), with no GPUI in it. The
//! turns of a thread are flattened into one list of rows: a turn header, the
//! prompt, the texts, the tool-call groups, the calls, their bodies, the edits and
//! their patches, and the markers at either end. The list draws the rows and
//! measures their heights; this module decides what they are and what changed.
//!
//! A run of tool calls in a turn is a group. It is closed until it is opened, and
//! its header counts the calls, names the tools and says how many edits and
//! failures it holds, so a closed group still tells what is inside.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use gm_schema::{EditState, ThreadDetail, ThreadEdit};

use crate::threads::{
    self, Block, Call, CallResult, CallStatus, Message, MessageKind, Place, ThreadsPage, Turn,
};

/// How close to the end, or the start, of the rows drawn a page is asked for.
pub const NEAR: usize = 20;
/// The most tool names a group header names before it counts the rest.
const NAMES: usize = 4;

/// What a tool-call group is keyed by: the thread, the turn, and the harness id of its first call.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupKey {
    pub uid: String,
    pub turn: u32,
    pub source: String,
}

/// A row at either end of the thread that says what is before or after the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Marker {
    /// Entries come before the first one shown. `loading` while they are read.
    Earlier { loading: bool },
    /// The entries after the last one shown are being read.
    Loading,
    /// The thread has no entries after the last one shown.
    End,
}

/// One row of the thread shown. A row holds what it draws, so a change of any of it
/// is a change of the row, and the list re-measures it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    Turn {
        number: u32,
        collapsed: bool,
        focused: bool,
    },
    Prompt {
        turn: u32,
        entry: Option<i64>,
        text: String,
        focused: bool,
    },
    Message {
        entry: i64,
        kind: MessageKind,
        text: String,
        depth: usize,
        focused: bool,
        /// Whether the gutter names the speaker: the first reply after a prompt (see `label_speakers`).
        label: bool,
    },
    /// A tool result whose call is not loaded: one line with its tool and size, and its
    /// text under the line when expanded.
    Result {
        entry: i64,
        depth: usize,
        label: String,
        text: String,
        expanded: bool,
        focused: bool,
    },
    /// The header of a group: its label, and the failures it holds, if any.
    Group {
        key: GroupKey,
        open: bool,
        label: String,
        failed: Option<String>,
    },
    Call {
        entry: i64,
        depth: usize,
        tool: String,
        summary: String,
        status: CallStatus,
        duration_ms: Option<i64>,
        expanded: bool,
        focused: bool,
    },
    /// The arguments and the result of an open call.
    CallBody {
        entry: i64,
        depth: usize,
        args: Option<String>,
        result: Option<CallResult>,
    },
    Edit {
        id: i64,
        depth: usize,
        path: String,
        counts: String,
        state: &'static str,
        open: bool,
    },
    Patch {
        id: i64,
        depth: usize,
        patch: String,
    },
    Marker(Marker),
}

/// The text of a group header: `▸ 7 tool calls: Read ×3, Edit ×2, Bash ×2 · 2 edits`
/// (`▾` when open), and the failures apart, as `1 failed`, for the view to colour.
#[must_use]
pub fn group_label(calls: &[&Call], open: bool) -> (String, Option<String>) {
    let mut all = Vec::new();
    for call in calls {
        collect(call, &mut all);
    }
    let mut tools: BTreeMap<&str, usize> = BTreeMap::new();
    for call in &all {
        let tool = if call.tool.is_empty() {
            "tool"
        } else {
            call.tool.as_str()
        };
        *tools.entry(tool).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = tools.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut names: Vec<String> = ranked
        .iter()
        .take(NAMES)
        .map(|(tool, count)| format!("{tool} ×{count}"))
        .collect();
    if ranked.len() > NAMES {
        names.push(format!("+{} more", ranked.len() - NAMES));
    }
    let noun = if all.len() == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    let chevron = if open { "▾" } else { "▸" };
    let mut label = format!("{chevron} {} {noun}: {}", all.len(), names.join(", "));
    let edits: usize = all.iter().map(|call| call.edits.len()).sum();
    if edits > 0 {
        let count = u32::try_from(edits).unwrap_or(u32::MAX);
        label.push_str(" · ");
        label.push_str(&threads::edits_label(count));
    }
    let failed = all
        .iter()
        .filter(|call| call.status == CallStatus::Failed)
        .count();
    let failed = (failed > 0).then(|| format!("{failed} failed"));
    (label, failed)
}

/// Every call of a group, and the calls its sub-agents made, in order.
fn collect<'a>(call: &'a Call, out: &mut Vec<&'a Call>) {
    out.push(call);
    for child in &call.children {
        if let Block::Call(child) = child {
            collect(child, out);
        }
    }
}

/// The runs of consecutive calls in a turn's blocks. A text ends a run.
fn runs(blocks: &[Block]) -> Vec<Vec<&Call>> {
    let mut out = Vec::new();
    let mut run: Vec<&Call> = Vec::new();
    for block in blocks {
        match block {
            Block::Call(call) => run.push(call),
            Block::Message(_) => {
                if !run.is_empty() {
                    out.push(std::mem::take(&mut run));
                }
            }
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out
}

/// The keys of every group of the thread, in order.
#[must_use]
pub fn group_keys(detail: &ThreadDetail) -> Vec<GroupKey> {
    let uid = detail.summary.uid.as_str();
    threads::turns(detail)
        .iter()
        .flat_map(|turn| {
            runs(&turn.blocks).into_iter().map(move |run| GroupKey {
                uid: uid.to_owned(),
                turn: turn.number,
                source: run[0].source_id.clone(),
            })
        })
        .collect()
}

/// The key of the group that holds entry `entry`, a call or anything a call made.
#[must_use]
pub fn group_of(detail: &ThreadDetail, entry: i64) -> Option<GroupKey> {
    let uid = detail.summary.uid.as_str();
    for turn in threads::turns(detail) {
        for run in runs(&turn.blocks) {
            let mut all = Vec::new();
            for call in &run {
                collect(call, &mut all);
            }
            if all.iter().any(|call| call.entry_id == entry) {
                return Some(GroupKey {
                    uid: uid.to_owned(),
                    turn: turn.number,
                    source: run[0].source_id.clone(),
                });
            }
        }
    }
    None
}

/// The rows of the thread shown, from its turns, with the state the page holds.
#[must_use]
pub fn flatten(page: &ThreadsPage) -> Vec<Row> {
    let Some(detail) = page.detail() else {
        return Vec::new();
    };
    let loading = page.loading();
    let mut rows = Vec::new();
    if detail.entries.first().is_some_and(|entry| entry.seq > 0) {
        rows.push(Row::Marker(Marker::Earlier {
            loading: loading == Some(Place::Earlier),
        }));
    }
    let focus = page.focus();
    let focus_turn = page.focus_turn();
    for turn in threads::turns(detail) {
        let collapsed = page.is_collapsed(turn.number);
        rows.push(Row::Turn {
            number: turn.number,
            collapsed,
            focused: focus_turn == Some(turn.number),
        });
        if collapsed {
            continue;
        }
        let shown = Shown {
            page,
            uid: detail.summary.uid.as_str(),
            turn: turn.number,
            focus,
        };
        push_turn(&mut rows, &turn, &shown);
    }
    match (loading, detail.has_more) {
        (Some(Place::Later), _) => rows.push(Row::Marker(Marker::Loading)),
        (_, false) => rows.push(Row::Marker(Marker::End)),
        _ => {}
    }
    label_speakers(&mut rows);
    rows
}

/// Marks the replies that name their speaker: the first reply of each run of replies,
/// a run that a prompt starts. The later replies of a run keep the gutter empty.
fn label_speakers(rows: &mut [Row]) {
    let mut in_reply = false;
    for row in rows {
        match row {
            Row::Prompt { .. } => in_reply = false,
            Row::Message {
                kind: MessageKind::Assistant,
                label,
                ..
            } => {
                *label = !in_reply;
                in_reply = true;
            }
            _ => {}
        }
    }
}

/// What a turn's rows are drawn from: the page, the thread, the turn and the focus.
struct Shown<'a> {
    page: &'a ThreadsPage,
    uid: &'a str,
    turn: u32,
    focus: Option<i64>,
}

fn is_focus(focus: Option<i64>, entry: i64) -> bool {
    focus == Some(entry)
}

fn push_turn(rows: &mut Vec<Row>, turn: &Turn, shown: &Shown<'_>) {
    if let Some(text) = &turn.prompt {
        rows.push(Row::Prompt {
            turn: turn.number,
            entry: turn.prompt_id,
            text: text.clone(),
            focused: turn
                .prompt_id
                .is_some_and(|entry| is_focus(shown.focus, entry)),
        });
    }
    let mut run: Vec<&Call> = Vec::new();
    for block in &turn.blocks {
        match block {
            Block::Call(call) => run.push(call),
            Block::Message(message) => {
                push_group(rows, &mut run, shown);
                push_message(rows, message, 0, shown);
            }
        }
    }
    push_group(rows, &mut run, shown);
    for edit in &turn.edits {
        push_edit(rows, edit, 0, shown.page);
    }
}

/// The header of a run of calls, and its calls when the group is open.
fn push_group(rows: &mut Vec<Row>, run: &mut Vec<&Call>, shown: &Shown<'_>) {
    let Some(first) = run.first() else {
        return;
    };
    let key = GroupKey {
        uid: shown.uid.to_owned(),
        turn: shown.turn,
        source: first.source_id.clone(),
    };
    let open = shown.page.is_group_open(&key);
    let (label, failed) = group_label(run, open);
    rows.push(Row::Group {
        key,
        open,
        label,
        failed,
    });
    if open {
        for call in run.iter() {
            push_call(rows, call, 0, shown);
        }
    }
    run.clear();
}

fn push_call(rows: &mut Vec<Row>, call: &Call, depth: usize, shown: &Shown<'_>) {
    let expanded = shown.page.is_expanded(call.entry_id);
    rows.push(Row::Call {
        entry: call.entry_id,
        depth,
        tool: call.tool.clone(),
        summary: call.summary.clone(),
        status: call.status,
        duration_ms: call.duration_ms,
        expanded,
        focused: is_focus(shown.focus, call.entry_id),
    });
    if expanded {
        rows.push(Row::CallBody {
            entry: call.entry_id,
            depth,
            args: call.args.clone(),
            result: call.result.clone(),
        });
    }
    for edit in &call.edits {
        push_edit(rows, edit, depth, shown.page);
    }
    for child in &call.children {
        match child {
            Block::Call(child) => push_call(rows, child, depth + 1, shown),
            Block::Message(message) => push_message(rows, message, depth + 1, shown),
        }
    }
}

/// A text of a turn: a reply or a notice as it reads, a result whose call is not
/// loaded as a line that a click opens.
fn push_message(rows: &mut Vec<Row>, message: &Message, depth: usize, shown: &Shown<'_>) {
    let focused = is_focus(shown.focus, message.entry_id);
    if message.kind == MessageKind::Result {
        rows.push(Row::Result {
            entry: message.entry_id,
            depth,
            label: result_label(message.tool.as_deref(), message.bytes),
            text: message.text.clone(),
            expanded: shown.page.is_expanded(message.entry_id),
            focused,
        });
    } else {
        rows.push(Row::Message {
            entry: message.entry_id,
            kind: message.kind,
            text: message.text.clone(),
            depth,
            focused,
            label: false,
        });
    }
}

/// The line of an unpaired result: its tool and its size, as `Edit result · 64 KB`.
fn result_label(tool: Option<&str>, bytes: u64) -> String {
    let size = threads::size_label(bytes);
    match tool.filter(|tool| !tool.is_empty()) {
        Some(tool) => format!("{tool} result · {size}"),
        None => format!("Result · {size}"),
    }
}

fn push_edit(rows: &mut Vec<Row>, edit: &ThreadEdit, depth: usize, page: &ThreadsPage) {
    let open = page.open_edit() == Some(edit.id);
    let state: Option<EditState> = page.edit_state(edit.id);
    rows.push(Row::Edit {
        id: edit.id,
        depth,
        path: edit.path.clone(),
        counts: threads::counts_label(edit.added, edit.removed),
        state: threads::edit_state_label(edit, state),
        open,
    });
    if open {
        rows.push(Row::Patch {
            id: edit.id,
            depth,
            patch: edit.patch.clone(),
        });
    }
}

/// The entry a row is about, for the rows that name one.
#[must_use]
pub fn entry_of(row: &Row) -> Option<i64> {
    match row {
        Row::Prompt { entry, .. } => *entry,
        Row::Message { entry, .. }
        | Row::Result { entry, .. }
        | Row::Call { entry, .. }
        | Row::CallBody { entry, .. } => Some(*entry),
        _ => None,
    }
}

/// The row of entry `entry`, the first one that names it.
#[must_use]
pub fn focus_row(rows: &[Row], entry: i64) -> Option<usize> {
    rows.iter().position(|row| entry_of(row) == Some(entry))
}

/// A hash of a row: what the list compares to find the rows that changed.
#[must_use]
pub fn key_of(row: &Row) -> u64 {
    let mut hasher = DefaultHasher::new();
    row.hash(&mut hasher);
    hasher.finish()
}

/// The span of the old rows that the new ones replace, and how many new rows go
/// there. The rows before and after the span are the same in both lists, so the
/// list keeps their measured heights and its scroll position above the span.
#[must_use]
pub fn splice_span(old: &[u64], new: &[u64]) -> (Range<usize>, usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let limit = old.len().min(new.len()) - prefix;
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(limit);
    let old_end = old.len() - suffix;
    let count = new.len() - suffix - prefix;
    (prefix..old_end, count)
}

/// Whether row `ix` of `count` rows is near the end: a page after the rows is asked for then.
#[must_use]
pub fn near_end(ix: usize, count: usize) -> bool {
    ix + NEAR >= count
}

/// Whether row `ix` is near the start: the entries before the rows are asked for then.
#[must_use]
pub fn near_start(ix: usize) -> bool {
    ix < NEAR
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::{CallStatus, Place};
    use gm_schema::{EditChange, EntryKind, ThreadEdit, ThreadEntry, ThreadSummary};

    fn summary(uid: &str) -> ThreadSummary {
        ThreadSummary {
            uid: uid.to_owned(),
            harness: "claude-code".to_owned(),
            harness_version: None,
            session_id: "s".to_owned(),
            worktree: "/repo".to_owned(),
            branch: None,
            model: None,
            title: "t".to_owned(),
            note: String::new(),
            starred: false,
            tags: Vec::new(),
            started_at: 0,
            last_event_at: 1_000,
            ended_at: None,
            entry_count: 0,
            edit_count: 0,
            redaction_count: 0,
            workspace: None,
        }
    }

    fn entry(id: i64, seq: u32, turn: u32, kind: EntryKind, source: &str) -> ThreadEntry {
        ThreadEntry {
            id,
            seq,
            turn,
            kind,
            source_id: source.to_owned(),
            parent_id: None,
            tool: None,
            text: String::new(),
            args_json: None,
            is_error: false,
            truncated: false,
            orig_bytes: None,
            redactions: 0,
            at: 1_000 * i64::from(seq),
        }
    }

    fn call(id: i64, seq: u32, turn: u32, source: &str, tool: &str) -> ThreadEntry {
        ThreadEntry {
            tool: Some(tool.to_owned()),
            args_json: Some(r#"{"command":"ls"}"#.to_owned()),
            ..entry(id, seq, turn, EntryKind::ToolCall, source)
        }
    }

    fn result(id: i64, seq: u32, turn: u32, call: &str, error: bool) -> ThreadEntry {
        ThreadEntry {
            text: "out".to_owned(),
            is_error: error,
            ..entry(
                id,
                seq,
                turn,
                EntryKind::ToolResult,
                &format!("{call}:result"),
            )
        }
    }

    fn text(id: i64, seq: u32, turn: u32, kind: EntryKind, body: &str) -> ThreadEntry {
        ThreadEntry {
            text: body.to_owned(),
            ..entry(id, seq, turn, kind, &format!("m{id}"))
        }
    }

    fn edit(id: i64, entry_id: i64) -> ThreadEdit {
        ThreadEdit {
            member: None,
            id,
            entry_id,
            path: "src/a.rs".to_owned(),
            old_path: None,
            change: EditChange::Modify,
            patch: "@@ -1 +1 @@\n-a\n+b".to_owned(),
            added: 1,
            removed: 1,
            pre_oid: None,
            post_oid: None,
            from_input: false,
            at: 1,
        }
    }

    fn detail(entries: Vec<ThreadEntry>, edits: Vec<ThreadEdit>, more: bool) -> ThreadDetail {
        ThreadDetail {
            summary: summary("u1"),
            parent: None,
            entries,
            edits,
            links: Vec::new(),
            relations: Vec::new(),
            redactions: Vec::new(),
            has_more: more,
        }
    }

    /// A page with `detail` loaded as the selection, and nothing read in flight.
    fn shown(detail: ThreadDetail) -> ThreadsPage {
        let mut page = ThreadsPage::default();
        let uid = detail.summary.uid.clone();
        let _ = page.select(&uid, None);
        let first = detail.entries.first().map_or(0, |entry| entry.seq);
        page.sent(
            &threads::Read::Detail {
                uid: uid.clone(),
                from_seq: first,
                place: Place::Replace,
            },
            gm_schema::OperationId(9),
        );
        let _ = page.read(gm_schema::OperationId(9), detail);
        page
    }

    fn kinds(rows: &[Row]) -> Vec<&'static str> {
        rows.iter()
            .map(|row| match row {
                Row::Turn { .. } => "turn",
                Row::Prompt { .. } => "prompt",
                Row::Message { .. } => "message",
                Row::Result { .. } => "result",
                Row::Group { .. } => "group",
                Row::Call { .. } => "call",
                Row::CallBody { .. } => "body",
                Row::Edit { .. } => "edit",
                Row::Patch { .. } => "patch",
                Row::Marker(_) => "marker",
            })
            .collect()
    }

    #[test]
    fn only_the_first_reply_after_a_prompt_names_its_speaker() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "fix it"),
            text(2, 1, 1, EntryKind::Assistant, "reading"),
            call(3, 2, 1, "c1", "Read"),
            result(4, 3, 1, "c1", false),
            text(5, 4, 1, EntryKind::Assistant, "done"),
            text(6, 5, 2, EntryKind::User, "again"),
            text(7, 6, 2, EntryKind::Assistant, "ok"),
        ];
        let page = shown(detail(entries, Vec::new(), false));
        let labels: Vec<bool> = page
            .rows()
            .iter()
            .filter_map(|row| match row {
                Row::Message { label, .. } => Some(*label),
                _ => None,
            })
            .collect();
        assert_eq!(labels, [true, false, true], "reading, done, ok");
    }

    #[test]
    fn a_turn_flattens_to_its_header_prompt_text_group_and_its_calls_only_when_open() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "fix it"),
            text(2, 1, 1, EntryKind::Assistant, "reading"),
            call(3, 2, 1, "c1", "Read"),
            result(4, 3, 1, "c1", false),
            call(5, 4, 1, "c2", "Bash"),
            result(6, 5, 1, "c2", false),
            text(7, 6, 1, EntryKind::Assistant, "done"),
        ];
        let page = shown(detail(entries, Vec::new(), false));
        assert_eq!(
            kinds(page.rows()),
            vec!["turn", "prompt", "message", "group", "message", "marker"],
            "the group is closed by default, so its calls are not rows"
        );
        let key = page
            .rows()
            .iter()
            .find_map(|row| match row {
                Row::Group { key, .. } => Some(key.clone()),
                _ => None,
            })
            .expect("a group");
        assert_eq!(key.source, "c1", "keyed by the first call's source id");
        let mut page = page;
        page.toggle_group(&key);
        assert_eq!(
            kinds(page.rows()),
            vec![
                "turn", "prompt", "message", "group", "call", "call", "message", "marker"
            ]
        );
    }

    #[test]
    fn a_closed_group_header_counts_its_calls_names_and_edits_and_failures() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Read"),
            result(3, 2, 1, "c1", false),
            call(4, 3, 1, "c2", "Edit"),
            result(5, 4, 1, "c2", true),
            call(6, 5, 1, "c3", "Read"),
            result(7, 6, 1, "c3", false),
            call(8, 7, 1, "c4", "Edit"),
            result(9, 8, 1, "c4", false),
            call(10, 9, 1, "c5", "Bash"),
            result(11, 10, 1, "c5", false),
        ];
        let edits = vec![edit(1, 4), edit(2, 8)];
        let page = shown(detail(entries, edits, false));
        let label = page
            .rows()
            .iter()
            .find_map(|row| match row {
                Row::Group { label, failed, .. } => Some((label.clone(), failed.clone())),
                _ => None,
            })
            .expect("a group");
        assert_eq!(
            label,
            (
                "▸ 5 tool calls: Edit ×2, Read ×2, Bash ×1 · 2 edits".to_owned(),
                Some("1 failed".to_owned())
            )
        );
    }

    #[test]
    fn a_group_names_at_most_four_tools_and_counts_the_rest() {
        let calls: Vec<Call> = ["A", "B", "C", "D", "E", "E"]
            .iter()
            .enumerate()
            .map(|(i, tool)| Call {
                entry_id: i64::try_from(i).unwrap_or(0),
                source_id: format!("s{i}"),
                tool: (*tool).to_owned(),
                summary: String::new(),
                args: None,
                result: None,
                status: CallStatus::Done,
                duration_ms: None,
                edits: Vec::new(),
                children: Vec::new(),
            })
            .collect();
        let refs: Vec<&Call> = calls.iter().collect();
        let (label, failed) = group_label(&refs, true);
        assert_eq!(label, "▾ 6 tool calls: E ×2, A ×1, B ×1, C ×1, +1 more");
        assert_eq!(failed, None);
    }

    #[test]
    fn a_single_call_group_is_singular() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Read"),
        ];
        let page = shown(detail(entries, Vec::new(), false));
        let label = page.rows().iter().find_map(|row| match row {
            Row::Group { label, .. } => Some(label.clone()),
            _ => None,
        });
        assert_eq!(label.as_deref(), Some("▸ 1 tool call: Read ×1"));
    }

    #[test]
    fn expand_and_collapse_all_open_and_close_every_group_of_the_thread() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "one"),
            call(2, 1, 1, "c1", "Read"),
            text(3, 2, 1, EntryKind::Assistant, "between"),
            call(4, 3, 1, "c2", "Bash"),
            text(5, 4, 2, EntryKind::User, "two"),
            call(6, 5, 2, "c3", "Edit"),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        page.expand_all_groups();
        let groups = kinds(page.rows())
            .iter()
            .filter(|kind| **kind == "group")
            .count();
        assert_eq!(groups, 3, "two groups in turn 1 and one in turn 2");
        assert_eq!(
            kinds(page.rows())
                .iter()
                .filter(|kind| **kind == "call")
                .count(),
            3
        );
        page.collapse_all_groups();
        assert!(!kinds(page.rows()).contains(&"call"));
        assert_eq!(
            kinds(page.rows())
                .iter()
                .filter(|kind| **kind == "group")
                .count(),
            3
        );
    }

    #[test]
    fn an_open_group_keeps_its_state_when_the_entries_after_it_change() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Read"),
            result(3, 2, 1, "c1", false),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        let key = group_keys(page.detail().expect("shown")).remove(0);
        page.toggle_group(&key);
        assert!(page.is_group_open(&key));
        let before = kinds(page.rows()).len();
        // A later page of the same turn adds a call to the group: it stays open.
        page.sent(
            &threads::Read::Detail {
                uid: "u1".to_owned(),
                from_seq: 3,
                place: Place::Later,
            },
            gm_schema::OperationId(10),
        );
        let more = detail(vec![call(4, 3, 1, "c2", "Bash")], Vec::new(), false);
        assert!(page.read(gm_schema::OperationId(10), more));
        assert!(page.is_group_open(&key));
        assert!(kinds(page.rows()).len() > before);
    }

    #[test]
    fn a_failed_call_and_an_edit_are_shown_in_the_open_group_with_their_patch_when_open() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Edit"),
            result(3, 2, 1, "c1", false),
        ];
        let mut page = shown(detail(entries, vec![edit(7, 2)], false));
        let key = group_keys(page.detail().expect("shown")).remove(0);
        page.toggle_group(&key);
        assert_eq!(
            kinds(page.rows()),
            vec!["turn", "prompt", "group", "call", "edit", "marker"]
        );
        page.toggle_diff(7);
        assert!(kinds(page.rows()).contains(&"patch"));
        page.toggle_diff(7);
        assert!(!kinds(page.rows()).contains(&"patch"));
    }

    #[test]
    fn an_expanded_call_has_a_body_row_and_a_collapsed_one_does_not() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Bash"),
            result(3, 2, 1, "c1", false),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        let key = group_keys(page.detail().expect("shown")).remove(0);
        page.toggle_group(&key);
        assert!(!kinds(page.rows()).contains(&"body"));
        page.toggle_call(2);
        assert!(kinds(page.rows()).contains(&"body"));
    }

    #[test]
    fn a_result_of_a_call_in_an_earlier_turn_is_shown_with_its_call_and_not_in_its_turn() {
        let entries = vec![
            text(1, 0, 4, EntryKind::User, "go"),
            call(2, 1, 4, "c1", "Edit"),
            text(3, 2, 5, EntryKind::User, "next"),
            result(4, 3, 5, "c1", false),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        let key = group_keys(page.detail().expect("shown")).remove(0);
        page.toggle_group(&key);
        assert!(!kinds(page.rows()).contains(&"result"));
        assert!(!kinds(page.rows()).contains(&"message"));
        let call_row = page
            .rows()
            .iter()
            .find(|row| matches!(row, Row::Call { .. }))
            .expect("the call row");
        assert!(matches!(
            call_row,
            Row::Call {
                status: CallStatus::Done,
                ..
            }
        ));
    }

    #[test]
    fn an_unpaired_result_is_one_collapsed_line_with_its_tool_and_size_until_it_is_opened() {
        let big = ThreadEntry {
            tool: Some("Edit".to_owned()),
            text: "{\"patch\":\"...\"}".to_owned(),
            orig_bytes: Some(65_563),
            ..result(2, 1, 1, "gone", false)
        };
        let entries = vec![text(1, 0, 1, EntryKind::User, "go"), big];
        let mut page = shown(detail(entries, Vec::new(), false));
        let row = |page: &ThreadsPage| {
            page.rows()
                .iter()
                .find(|row| matches!(row, Row::Result { .. }))
                .cloned()
                .expect("the result row")
        };
        assert!(matches!(
            row(&page),
            Row::Result {
                entry: 2,
                depth: 0,
                ref label,
                expanded: false,
                ..
            } if label == "Edit result · 64 KB"
        ));
        assert!(!kinds(page.rows()).contains(&"message"));
        page.toggle_call(2);
        assert!(matches!(row(&page), Row::Result { expanded: true, .. }));
        page.toggle_call(2);
        assert!(matches!(
            row(&page),
            Row::Result {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn a_result_line_names_its_tool_or_says_result_when_the_tool_is_unknown() {
        assert_eq!(result_label(Some("Edit"), 65_563), "Edit result · 64 KB");
        assert_eq!(result_label(Some(""), 1), "Result · 1 B");
        assert_eq!(result_label(None, 1_048_576), "Result · 1 MB");
    }

    #[test]
    fn a_collapsed_turn_keeps_only_its_header() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            text(2, 1, 1, EntryKind::Assistant, "ok"),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        page.toggle_turn(1);
        assert_eq!(kinds(page.rows()), vec!["turn", "marker"]);
    }

    #[test]
    fn the_thread_ends_with_end_when_done_and_loading_while_a_page_is_read() {
        let entries = vec![text(1, 0, 1, EntryKind::User, "go")];
        let mut page = shown(detail(entries.clone(), Vec::new(), true));
        assert_eq!(
            kinds(page.rows()),
            vec!["turn", "prompt"],
            "more follow: no marker yet"
        );
        let request = page.plan_more().expect("a page after the rows");
        page.sent(&request.read, gm_schema::OperationId(4));
        assert_eq!(page.rows().last(), Some(&Row::Marker(Marker::Loading)));
        assert!(page.read(
            gm_schema::OperationId(4),
            detail(
                vec![text(2, 1, 1, EntryKind::Assistant, "x")],
                Vec::new(),
                false
            )
        ));
        assert_eq!(page.rows().last(), Some(&Row::Marker(Marker::End)));
    }

    #[test]
    fn a_window_that_starts_after_zero_has_an_earlier_marker_on_top() {
        let entries = vec![text(5, 20, 1, EntryKind::User, "hit")];
        let page = shown(detail(entries, Vec::new(), false));
        assert_eq!(
            page.rows().first(),
            Some(&Row::Marker(Marker::Earlier { loading: false }))
        );
        let request = page.plan_earlier().expect("earlier entries");
        assert!(matches!(
            request.kind,
            gm_schema::CommandKind::ReadThread {
                from_seq: 0,
                limit: 20,
                ..
            }
        ));
    }

    #[test]
    fn a_hit_opens_its_entry_as_the_focus_row_of_the_list() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            text(2, 1, 1, EntryKind::Assistant, "a"),
            text(3, 2, 1, EntryKind::Assistant, "needle"),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        let _ = page.select("u1", Some((3, 2)));
        let rows = page.rows();
        let ix = focus_row(rows, 3).expect("the hit is a row");
        assert_eq!(
            rows[ix],
            Row::Message {
                entry: 3,
                kind: MessageKind::Assistant,
                text: "needle".to_owned(),
                depth: 0,
                focused: true,
                label: false,
            }
        );
    }

    #[test]
    fn a_hit_inside_a_closed_group_opens_the_group_so_its_row_exists() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            call(2, 1, 1, "c1", "Read"),
            result(3, 2, 1, "c1", false),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        let _ = page.select("u1", Some((2, 1)));
        assert!(focus_row(page.rows(), 2).is_some());
    }

    #[test]
    fn a_hit_inside_a_collapsed_turn_opens_the_turn() {
        let entries = vec![
            text(1, 0, 1, EntryKind::User, "go"),
            text(2, 1, 1, EntryKind::Assistant, "hit"),
        ];
        let mut page = shown(detail(entries, Vec::new(), false));
        page.toggle_turn(1);
        let _ = page.select("u1", Some((2, 1)));
        assert!(!page.is_collapsed(1));
        assert!(focus_row(page.rows(), 2).is_some());
    }

    #[test]
    fn the_prompt_of_a_hit_is_the_focus_row_too() {
        let entries = vec![text(1, 0, 1, EntryKind::User, "find me")];
        let mut page = shown(detail(entries, Vec::new(), false));
        let _ = page.select("u1", Some((1, 0)));
        assert!(
            page.rows()
                .iter()
                .any(|row| matches!(row, Row::Prompt { focused: true, .. }))
        );
    }

    #[test]
    fn splice_span_keeps_the_rows_before_and_after_the_change() {
        assert_eq!(splice_span(&[1, 2, 3], &[1, 2, 3]), (3..3, 0));
        assert_eq!(
            splice_span(&[1, 2, 3], &[1, 2, 3, 4]),
            (3..3, 1),
            "an append"
        );
        assert_eq!(
            splice_span(&[1, 2, 3], &[0, 1, 2, 3]),
            (0..0, 1),
            "a prepend"
        );
        assert_eq!(
            splice_span(&[1, 2, 3], &[1, 9, 3]),
            (1..2, 1),
            "one row changed"
        );
        assert_eq!(
            splice_span(&[1, 2, 3], &[1, 3]),
            (1..2, 0),
            "one row removed"
        );
        assert_eq!(splice_span(&[], &[1, 2]), (0..0, 2));
        assert_eq!(splice_span(&[1, 2], &[]), (0..2, 0));
    }

    #[test]
    fn a_row_hash_is_the_same_for_the_same_row_and_differs_for_a_change() {
        let a = Row::Marker(Marker::Loading);
        assert_eq!(key_of(&a), key_of(&Row::Marker(Marker::Loading)));
        assert_ne!(key_of(&a), key_of(&Row::Marker(Marker::End)));
    }

    #[test]
    fn paging_is_asked_within_twenty_rows_of_either_end() {
        assert!(near_end(80, 100));
        assert!(!near_end(79, 100));
        assert!(near_end(0, 5), "a short list is all near its end");
        assert!(near_start(19));
        assert!(!near_start(20));
    }
}
