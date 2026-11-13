//! The Blame and Origin screens (D241, M5). Blame shows who last changed each line of a file
//! in the working tree, one section per run of lines of one commit, or **not committed**.
//! Origin answers how some lines got where they are, with the commits, the agent threads and
//! their evidence, as the app's provenance panel words them (`gm_view::provenance_view`).

use gm_view::provenance_view::Row;

use crate::data::Repo;
use crate::nav::sections::Section;
use crate::status::Item;

/// The Blame screen's sections for `path`.
pub fn blame(repo: &Repo, path: &str) -> Vec<Section<Item>> {
    let mut out = vec![Section::leaf(
        "head",
        Item::Header {
            label: "Blame:",
            text: path.to_owned(),
        },
    )];
    let Some((_, lines)) = repo.blame.as_ref().filter(|(of, _)| of == path) else {
        out.push(Section::leaf(
            "reading",
            Item::Note("Reading who changed each line…".to_owned()),
        ));
        return out;
    };
    if lines.is_empty() {
        out.push(Section::leaf(
            "empty",
            Item::Note("The file is empty.".to_owned()),
        ));
    }
    let mut first = 0;
    while first < lines.len() {
        let commit = &lines[first].commit;
        let last = (first..lines.len())
            .take_while(|&i| &lines[i].commit == commit)
            .last()
            .unwrap_or(first);
        let children = (first..=last)
            .map(|index| {
                Section::leaf(
                    format!("line/{}", lines[index].line),
                    Item::BlameLine { index },
                )
            })
            .collect();
        out.push(Section::with(
            format!("run/{}", lines[first].line),
            Item::BlameRun { first, last },
            children,
        ));
        first = last + 1;
    }
    out
}

/// The Origin screen's sections: each commit, its threads under it and what each thread
/// carries under the thread.
pub fn origin(repo: &Repo) -> Vec<Section<Item>> {
    let mut out = Vec::new();
    if let Some(asked) = repo.origin.asked() {
        let lines = if asked.from == asked.to {
            format!("line {}", asked.from)
        } else {
            format!("lines {}–{}", asked.from, asked.to)
        };
        out.push(Section::leaf(
            "head",
            Item::Header {
                label: "Origin:",
                text: format!("{} {lines}", asked.path),
            },
        ));
    }
    // Where the next row goes: under the last commit, or under its last thread.
    let mut commits: Vec<Section<Item>> = Vec::new();
    let mut top: Vec<Section<Item>> = Vec::new();
    for (index, row) in repo.origin.rows().into_iter().enumerate() {
        let id = format!("row/{index}");
        match &row {
            Row::Commit { .. } => commits.push(Section::with(id, Item::Origin(row), Vec::new())),
            Row::Thread { .. } => match commits.last_mut() {
                Some(commit) => {
                    commit
                        .children
                        .push(Section::with(id, Item::Origin(row), Vec::new()));
                }
                None => top.push(Section::leaf(id, Item::Origin(row))),
            },
            Row::Note(_) | Row::Failure(_) | Row::Cut(_) if commits.is_empty() => {
                top.push(Section::leaf(id, Item::Origin(row)));
            }
            Row::Cut(_) => commits.push(Section::leaf(id, Item::Origin(row))),
            _ => {
                let leaf = Section::leaf(id, Item::Origin(row));
                match commits.last_mut() {
                    Some(commit) => match commit.children.last_mut() {
                        Some(thread) if matches!(thread.item, Item::Origin(Row::Thread { .. })) => {
                            thread.children.push(leaf);
                        }
                        _ => commit.children.push(leaf),
                    },
                    None => top.push(leaf),
                }
            }
        }
    }
    out.extend(top);
    out.extend(commits);
    out
}
