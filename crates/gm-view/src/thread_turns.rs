//! The turns of a captured agent thread (T5), with no GPUI or terminal in it: each prompt
//! of the human with what the agent did until it stopped, each tool call paired with its
//! result, its edits and the calls of its sub-agent; and the small words the views share
//! for a call's argument, a duration, a patch's lines and a thread's files. Moved from
//! `gm-ui` so the terminal companion's Agent thread screen reads a thread as the app does
//! (D248).

use std::collections::{BTreeMap, HashMap, HashSet};

use gm_schema::{
    EntryKind, LinkKind, LinkOrigin, ThreadDetail, ThreadEdit, ThreadEntry, ThreadLink,
};
use serde_json::Value;

/// The longest an argument summary is, in characters, before it is cut.
pub const SUMMARY_CHARS: usize = 120;

/// The suffix the Claude Code adapter gives a tool result: `<call>:result`.
const RESULT_SUFFIX: &str = ":result";

/// One file of a thread's details panel: its path, the lines its edits added and
/// removed, and the manual link that names it, if the user made one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLine {
    pub path: String,
    /// Whether an edit of the thread changed the file, so its line counts are known.
    pub edited: bool,
    pub added: u32,
    pub removed: u32,
    pub manual: Option<ThreadLink>,
}

/// The files of a thread: each edited path once, with the lines of all its edits,
/// in the order they were first edited, then the files linked by hand that no edit names.
#[must_use]
pub fn file_lines(detail: &ThreadDetail) -> Vec<FileLine> {
    let mut lines: Vec<FileLine> = Vec::new();
    for edit in &detail.edits {
        match lines.iter_mut().find(|line| line.path == edit.path) {
            Some(line) => {
                line.added = line.added.saturating_add(edit.added);
                line.removed = line.removed.saturating_add(edit.removed);
            }
            None => lines.push(FileLine {
                path: edit.path.clone(),
                edited: true,
                added: edit.added,
                removed: edit.removed,
                manual: None,
            }),
        }
    }
    for link in &detail.links {
        if link.kind != LinkKind::File || link.origin != LinkOrigin::Human {
            continue;
        }
        match lines.iter_mut().find(|line| line.path == link.reference) {
            Some(line) => line.manual = Some(link.clone()),
            None => lines.push(FileLine {
                path: link.reference.clone(),
                edited: false,
                added: 0,
                removed: 0,
                manual: Some(link.clone()),
            }),
        }
    }
    lines
}

/// A duration as the call line shows it: `120 ms`, `2.5 s`, `3 min 4 s`.
#[must_use]
pub fn duration_label(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{}.{} s", ms / 1_000, (ms % 1_000) / 100)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1_000)
    }
}

/// One line of a patch, and how the view colours it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchKind {
    Added,
    Removed,
    Hunk,
    Context,
}

/// The lines of a unified diff, classed for the colours of the diff view.
#[must_use]
pub fn patch_lines(patch: &str) -> Vec<(PatchKind, &str)> {
    patch
        .lines()
        .map(|line| {
            let kind = if line.starts_with("@@") {
                PatchKind::Hunk
            } else if line.starts_with("+++")
                || line.starts_with("---")
                || line.starts_with("diff ")
                || line.starts_with("index ")
            {
                PatchKind::Context
            } else if line.starts_with('+') {
                PatchKind::Added
            } else if line.starts_with('-') {
                PatchKind::Removed
            } else {
                PatchKind::Context
            };
            (kind, line)
        })
        .collect()
}

/// The argument of a tool call, as one short line: its command, path, pattern, URL
/// or description, or else its first text value.
#[must_use]
pub fn argument_summary(args_json: Option<&str>) -> String {
    const KEYS: [&str; 6] = [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ];
    let Some(value) = args_json.and_then(|json| serde_json::from_str::<Value>(json).ok()) else {
        return String::new();
    };
    let text = KEYS
        .iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .or_else(|| value.as_object()?.values().find_map(Value::as_str));
    text.map_or_else(String::new, |text| one_line(text, SUMMARY_CHARS))
}

/// The text collapsed to one line of at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// How a tool call ended, as its row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallStatus {
    Done,
    Failed,
    /// No result has been recorded: the call was interrupted, or its result is on a later page.
    Pending,
}

/// The result of a tool call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallResult {
    pub text: String,
    /// Whether the entry was cut at the capture limit.
    pub truncated: bool,
}

/// A tool call of a turn, with its result, its edits and the calls its sub-agent made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// The entry id of the call: what the state and the expansion are keyed by.
    pub entry_id: i64,
    /// The harness's id of the call, which a tool-call group is keyed by.
    pub source_id: String,
    pub tool: String,
    pub summary: String,
    /// The arguments as stored, redacted.
    pub args: Option<String>,
    pub result: Option<CallResult>,
    pub status: CallStatus,
    pub duration_ms: Option<i64>,
    pub edits: Vec<ThreadEdit>,
    pub children: Vec<Block>,
}

/// The kind of a text entry of a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageKind {
    Assistant,
    Notice,
    /// A tool result whose call is not in the turn shown.
    Result,
}

/// A text entry of a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub entry_id: i64,
    pub kind: MessageKind,
    pub text: String,
    /// The native tool name of a tool result.
    pub tool: Option<String>,
    /// The size of the entry's text before the cap, in bytes.
    pub bytes: u64,
}

/// One row of a turn: a text, or a tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Message(Message),
    Call(Call),
}

/// One prompt of the human and everything the agent did until it stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub number: u32,
    pub prompt: Option<String>,
    /// The entry id of the prompt, which a search hit can name.
    pub prompt_id: Option<i64>,
    pub blocks: Vec<Block>,
    /// Edits of this turn whose call is not in it.
    pub edits: Vec<ThreadEdit>,
}

/// Groups the entries of a thread by turn, in order. Each tool result joins the call
/// whose `<call>:result` id it has, in the same turn or in an earlier one (a result
/// that arrives after the next prompt is stored in that later turn); a sub-agent's
/// entries go under the call that started them; and each edit goes under the call
/// entry that carried it.
#[must_use]
pub fn turns(detail: &ThreadDetail) -> Vec<Turn> {
    let mut ordered: Vec<&ThreadEntry> = detail.entries.iter().collect();
    ordered.sort_by_key(|entry| (entry.turn, entry.seq));
    let pairing = Pairing::of(&ordered);
    let mut by_turn: BTreeMap<u32, Vec<&ThreadEntry>> = BTreeMap::new();
    for entry in ordered {
        by_turn.entry(entry.turn).or_default().push(entry);
    }
    by_turn
        .into_iter()
        .map(|(number, entries)| build_turn(number, &entries, &detail.edits, &pairing))
        .collect()
}

/// The results of the thread that have a call in the thread loaded, by call id.
struct Pairing<'a> {
    by_call: HashMap<&'a str, &'a ThreadEntry>,
    /// The entry ids of the results that are paired, so their turn does not show them.
    paired: HashSet<i64>,
}

impl<'a> Pairing<'a> {
    /// Pairs over all the entries loaded, in turn and sequence order, so a result
    /// finds its call in any turn loaded.
    fn of(entries: &[&'a ThreadEntry]) -> Self {
        let calls: HashSet<&str> = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::ToolCall)
            .map(|entry| entry.source_id.as_str())
            .collect();
        let by_call: HashMap<&str, &ThreadEntry> = entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::ToolResult)
            .filter_map(|entry| {
                let call = entry.source_id.strip_suffix(RESULT_SUFFIX)?;
                calls.contains(call).then_some((call, *entry))
            })
            .collect();
        let paired = by_call.values().map(|result| result.id).collect();
        Self { by_call, paired }
    }
}

/// A block before its sub-agent entries are placed under it.
struct Node {
    /// The source id that other entries name as their parent.
    source: String,
    parent: Option<String>,
    block: Block,
}

fn build_turn(
    number: u32,
    entries: &[&ThreadEntry],
    edits: &[ThreadEdit],
    pairing: &Pairing<'_>,
) -> Turn {
    let prompt_entry = entries.iter().find(|entry| entry.kind == EntryKind::User);
    let prompt = prompt_entry.map(|entry| entry.text.clone());
    let prompt_id = prompt_entry.map(|entry| entry.id);
    let call_entries: HashSet<i64> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::ToolCall)
        .map(|entry| entry.id)
        .collect();
    let turn_entries: HashSet<i64> = entries.iter().map(|entry| entry.id).collect();
    let attached_edits = |entry: i64| -> Vec<ThreadEdit> {
        edits
            .iter()
            .filter(|edit| edit.entry_id == entry)
            .cloned()
            .collect()
    };

    let mut nodes: Vec<Node> = Vec::new();
    for entry in entries {
        let block = match entry.kind {
            EntryKind::User => continue,
            EntryKind::ToolResult if pairing.paired.contains(&entry.id) => continue,
            EntryKind::ToolResult => Block::Message(message(entry, MessageKind::Result)),
            EntryKind::Assistant => Block::Message(message(entry, MessageKind::Assistant)),
            EntryKind::Notice => Block::Message(message(entry, MessageKind::Notice)),
            EntryKind::ToolCall => {
                let result = pairing.by_call.get(entry.source_id.as_str()).copied();
                let duration_ms = result
                    .map(|result| result.at - entry.at)
                    .filter(|d| *d >= 0);
                let status = match result {
                    None => CallStatus::Pending,
                    Some(result) if result.is_error => CallStatus::Failed,
                    Some(_) => CallStatus::Done,
                };
                Block::Call(Call {
                    entry_id: entry.id,
                    source_id: entry.source_id.clone(),
                    tool: entry.tool.clone().unwrap_or_default(),
                    summary: argument_summary(entry.args_json.as_deref()),
                    args: entry.args_json.clone(),
                    result: result.map(|result| CallResult {
                        text: result.text.clone(),
                        truncated: result.truncated,
                    }),
                    status,
                    duration_ms,
                    edits: attached_edits(entry.id),
                    children: Vec::new(),
                })
            }
        };
        nodes.push(Node {
            source: entry.source_id.clone(),
            parent: entry.parent_id.clone(),
            block,
        });
    }

    let placed = place(nodes);
    let turn_edits = edits
        .iter()
        .filter(|edit| {
            turn_entries.contains(&edit.entry_id) && !call_entries.contains(&edit.entry_id)
        })
        .cloned()
        .collect();
    Turn {
        number,
        prompt,
        prompt_id,
        blocks: placed,
        edits: turn_edits,
    }
}

fn message(entry: &ThreadEntry, kind: MessageKind) -> Message {
    Message {
        entry_id: entry.id,
        kind,
        text: entry.text.clone(),
        tool: entry.tool.clone(),
        bytes: entry
            .orig_bytes
            .unwrap_or_else(|| u64::try_from(entry.text.len()).unwrap_or(u64::MAX)),
    }
}

/// Puts each block under the tool call its parent names, and returns the blocks
/// that have no parent in the turn, in order. Every block has one parent at most,
/// so the walk from the roots reaches each block once.
fn place(nodes: Vec<Node>) -> Vec<Block> {
    let (roots, children) = {
        let index: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| matches!(node.block, Block::Call(_)))
            .map(|(position, node)| (node.source.as_str(), position))
            .collect();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        for (position, node) in nodes.iter().enumerate() {
            match node.parent.as_deref().and_then(|parent| index.get(parent)) {
                Some(&parent) if parent != position => children[parent].push(position),
                _ => roots.push(position),
            }
        }
        (roots, children)
    };
    let mut blocks: Vec<Option<Block>> = nodes.into_iter().map(|node| Some(node.block)).collect();
    roots
        .into_iter()
        .filter_map(|root| assemble(root, &mut blocks, &children))
        .collect()
}

fn assemble(
    position: usize,
    blocks: &mut [Option<Block>],
    children: &[Vec<usize>],
) -> Option<Block> {
    let mut block = blocks.get_mut(position)?.take()?;
    if let Block::Call(call) = &mut block {
        call.children = children[position]
            .iter()
            .filter_map(|&child| assemble(child, blocks, children))
            .collect();
    }
    Some(block)
}
