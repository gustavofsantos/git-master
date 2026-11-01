//! Labels and thread-to-thread links in `threads.db` (design/threads.md, "Graph, provenance and
//! labels"; D143, D145). The engine decides who may write and what a commit snapshot holds; this
//! module stores the rows and checks what the store must hold: names, colours, the kind of each
//! reference and the links that cannot exist.

use gm_schema::threads::{
    AttachKind, Label, LabelAttachment, LabelDetail, ThreadRelKind, ThreadRelation,
};
use rusqlite::{OptionalExtension, Row, Transaction, params};

use super::store::{StoreError, ThreadStore, begin, invalid, thread_id};
use crate::CommitSnapshot;

/// Characters of a label name (D145). Names are unique and case-sensitive, as tags were.
pub const NAME_CHARS: usize = 64;
/// Characters of a label description.
pub const DESCRIPTION_CHARS: usize = 500;
/// Thread links a thread detail carries, newest first (D143).
pub const DETAIL_RELATIONS: u32 = 50;

/// The label's columns, in the order `label_row` reads them. `attachments` counts every kind.
const LABEL_COLUMNS: &str = "l.id, l.name, l.description, l.color, l.origin, l.created_at,
    (SELECT COUNT(*) FROM label_attach a WHERE a.label_id = l.id)";

impl ThreadStore {
    /// Labels whose name contains `query` (any case; empty for all), by name, at most `limit`.
    pub fn list_labels(&self, query: &str, limit: u32) -> Result<Vec<Label>, StoreError> {
        let sql = format!(
            "SELECT {LABEL_COLUMNS} FROM label l
             WHERE instr(lower(l.name), lower(?1)) > 0
             ORDER BY l.name LIMIT ?2"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let labels = statement
            .query_map(params![query, i64::from(limit)], label_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(labels)
    }

    /// Label `id` with its attachments, by kind and reference.
    pub fn read_label(&self, id: i64) -> Result<LabelDetail, StoreError> {
        let label = label_by_id(&self.conn, id)?;
        let mut statement = self.conn.prepare(
            "SELECT kind, ref, origin, at, subject, author, committed_at, resolved
             FROM label_attach WHERE label_id = ?1 ORDER BY kind, ref",
        )?;
        let attachments = statement
            .query_map(params![id], attachment_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(LabelDetail { label, attachments })
    }

    /// Creates label `name`, or returns the label of that name when there is one: names are
    /// unique, and the description and colour of an existing label are not changed.
    pub fn create_label(
        &mut self,
        name: &str,
        description: &str,
        color: Option<&str>,
        origin: &str,
        at: i64,
    ) -> Result<Label, StoreError> {
        let name = check_name(name)?;
        let description = check_description(description)?;
        let color = check_color(color)?;
        let tx = begin(&mut self.conn)?;
        let label = match label_by_name(&tx, &name)? {
            Some(existing) => existing,
            None => insert_label(&tx, &name, &description, color.as_deref(), origin, at)?,
        };
        tx.commit()?;
        Ok(label)
    }

    /// Attaches label `name` (created with `origin` when missing) to a reference of `kind`.
    /// A thread reference is a uid, a file is a repository path, a commit is the snapshot Git
    /// gave (its full id becomes the reference), a label is the id of another label. Attaching a
    /// pair already attached returns the attachment that is there.
    pub fn attach_label(
        &mut self,
        name: &str,
        kind: AttachKind,
        reference: &str,
        snapshot: Option<&CommitSnapshot>,
        origin: &str,
        at: i64,
    ) -> Result<(Label, LabelAttachment), StoreError> {
        let name = check_name(name)?;
        let tx = begin(&mut self.conn)?;
        let label = match label_by_name(&tx, &name)? {
            Some(existing) => existing,
            None => insert_label(&tx, &name, "", None, origin, at)?,
        };
        let reference = check_reference(&tx, kind, reference, snapshot, label.id)?;
        let (subject, author, committed_at) = match (kind, snapshot) {
            (AttachKind::Commit, Some(snap)) => (
                Some(snap.subject.as_str()),
                Some(snap.author.as_str()),
                Some(snap.committed_at),
            ),
            _ => (None, None, None),
        };
        tx.execute(
            "INSERT OR IGNORE INTO label_attach
                (label_id, kind, ref, origin, at, subject, author, committed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                label.id,
                attach_kind_text(kind),
                reference,
                origin,
                at,
                subject,
                author,
                committed_at
            ],
        )?;
        let attachment = tx.query_row(
            "SELECT kind, ref, origin, at, subject, author, committed_at, resolved
             FROM label_attach WHERE label_id = ?1 AND kind = ?2 AND ref = ?3",
            params![label.id, attach_kind_text(kind), reference],
            attachment_row,
        )??;
        let label = label_by_id(&tx, label.id)?;
        tx.commit()?;
        Ok((label, attachment))
    }

    /// Removes the attachment of label `name` to `kind` `reference`. `NotFound` when there is no
    /// such label or attachment.
    pub fn detach_label(
        &mut self,
        name: &str,
        kind: AttachKind,
        reference: &str,
    ) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let label = label_by_name(&tx, name)?.ok_or(StoreError::NotFound)?;
        let removed = tx.execute(
            "DELETE FROM label_attach WHERE label_id = ?1 AND kind = ?2 AND ref = ?3",
            params![label.id, attach_kind_text(kind), reference],
        )?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.commit()?;
        Ok(())
    }

    /// Renames label `id`. A name another label holds is refused.
    pub fn rename_label(&mut self, id: i64, name: &str) -> Result<Label, StoreError> {
        let name = check_name(name)?;
        let tx = begin(&mut self.conn)?;
        label_by_id(&tx, id)?;
        if let Some(other) = label_by_name(&tx, &name)?
            && other.id != id
        {
            return Err(invalid(&format!("a label named {name} already exists")));
        }
        tx.execute(
            "UPDATE label SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        let label = label_by_id(&tx, id)?;
        tx.commit()?;
        Ok(label)
    }

    /// Deletes label `id` with its attachments, and the attachments other labels made to it.
    pub fn delete_label(&mut self, id: i64) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        let removed = tx.execute("DELETE FROM label WHERE id = ?1", params![id])?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        tx.execute(
            "DELETE FROM label_attach WHERE kind = 'label' AND ref = CAST(?1 AS TEXT)",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Links thread `from` to thread `to` with `kind`. A link already there is returned as it is.
    pub fn link_threads(
        &mut self,
        from: &str,
        to: &str,
        kind: ThreadRelKind,
        origin: &str,
        at: i64,
    ) -> Result<ThreadRelation, StoreError> {
        if from == to {
            return Err(invalid("a thread cannot be linked to itself"));
        }
        let tx = begin(&mut self.conn)?;
        let (from_id, to_id) = relation_ends(&tx, from, to)?;
        tx.execute(
            "INSERT OR IGNORE INTO thread_rel (from_thread, to_thread, kind, origin, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![from_id, to_id, rel_kind_text(kind), origin, at],
        )?;
        let relation = relation_row(&tx, from_id, to_id, kind)?;
        tx.commit()?;
        Ok(relation)
    }

    /// Removes the link from `from` to `to` of `kind`, and returns what was removed. `NotFound`
    /// when there is none.
    pub fn remove_thread_relation(
        &mut self,
        from: &str,
        to: &str,
        kind: ThreadRelKind,
    ) -> Result<ThreadRelation, StoreError> {
        let tx = begin(&mut self.conn)?;
        let (from_id, to_id) = relation_ends(&tx, from, to)?;
        let relation = relation_row(&tx, from_id, to_id, kind)?;
        tx.execute(
            "DELETE FROM thread_rel WHERE from_thread = ?1 AND to_thread = ?2 AND kind = ?3",
            params![from_id, to_id, rel_kind_text(kind)],
        )?;
        tx.commit()?;
        Ok(relation)
    }

    /// Stores the snapshot of commit `commit` on every commit link that names it (D145). The
    /// stored id is not changed, and a link that already has a snapshot keeps it.
    pub fn set_commit_snapshot(
        &mut self,
        commit: &str,
        snapshot: &CommitSnapshot,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE thread_link SET subject = ?2, author = ?3, committed_at = ?4
             WHERE kind = 'commit' AND ref = ?1 AND subject IS NULL",
            params![
                commit,
                snapshot.subject,
                snapshot.author,
                snapshot.committed_at
            ],
        )?;
        Ok(())
    }

    /// The commit names of commit links with no snapshot yet, each once, so the engine can take
    /// their snapshots while the commits still exist.
    pub fn commit_refs_without_snapshot(&self) -> Result<Vec<String>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT ref FROM thread_link WHERE kind = 'commit' AND subject IS NULL
             ORDER BY ref",
        )?;
        let refs = statement
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(refs)
    }
}

impl ThreadStore {
    /// Records the restack's replays (D142, D145): each `(old, new)` pair makes every commit link
    /// and label attachment that names `old`, or resolves to it, resolve to `new`. The stored
    /// ids are not changed. Each pair is a cache entry, so a failed remap costs only the read's
    /// own check.
    pub fn remap_commits(&mut self, pairs: &[(String, String)]) -> Result<(), StoreError> {
        let tx = begin(&mut self.conn)?;
        for (old, new) in pairs {
            tx.execute(
                "UPDATE thread_link SET resolved = ?2
                 WHERE kind = 'commit' AND (ref = ?1 OR resolved = ?1)",
                params![old, new],
            )?;
            tx.execute(
                "UPDATE label_attach SET resolved = ?2
                 WHERE kind = 'commit' AND (ref = ?1 OR resolved = ?1)",
                params![old, new],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The commit links of thread `uid` with their cached resolution (D145), by commit id. A cache
    /// is a commit id, `ambiguous` or `gone`; `None` when nothing is cached yet.
    pub fn commit_link_cache(
        &self,
        uid: &str,
    ) -> Result<Vec<(String, Option<String>)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT l.ref, l.resolved FROM thread_link l JOIN thread t ON t.id = l.thread_id
             WHERE t.uid = ?1 AND l.kind = 'commit' ORDER BY l.ref",
        )?;
        let rows = statement
            .query_map(params![uid], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Stores the resolution of commit link `reference` of thread `uid` (D145): a commit id,
    /// `ambiguous` or `gone`. The stored id is not changed.
    pub fn set_link_cache(
        &mut self,
        uid: &str,
        reference: &str,
        value: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE thread_link SET resolved = ?3
             WHERE kind = 'commit' AND ref = ?2
               AND thread_id = (SELECT id FROM thread WHERE uid = ?1)",
            params![uid, reference, value],
        )?;
        Ok(())
    }

    /// Stores the resolution of commit attachment `reference` of label `label` (D145), the same
    /// values as `set_link_cache`.
    pub fn set_attach_cache(
        &mut self,
        label: i64,
        reference: &str,
        value: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE label_attach SET resolved = ?3 WHERE kind = 'commit' AND label_id = ?1 AND ref = ?2",
            params![label, reference, value],
        )?;
        Ok(())
    }

    /// The labels of thread `uid` as `(name, origin)`, by name, at most `limit` (D144, second edge).
    pub fn thread_labels(
        &self,
        uid: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT l.name, a.origin FROM label_attach a JOIN label l ON l.id = a.label_id
             WHERE a.kind = 'thread' AND a.ref = ?1 ORDER BY l.name LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![uid, i64::from(limit)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The thread-to-thread links of thread `uid` in either direction as whole rows, newest first,
    /// at most `limit` (D143). Both ends are uids, so the detail of a thread can say which way each
    /// link points and who made it.
    pub fn thread_relation_rows(
        &self,
        uid: &str,
        limit: u32,
    ) -> Result<Vec<ThreadRelation>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT f.uid, o.uid, r.kind, r.origin, r.at
             FROM thread_rel r
             JOIN thread f ON f.id = r.from_thread
             JOIN thread o ON o.id = r.to_thread
             WHERE f.uid = ?1 OR o.uid = ?1
             ORDER BY r.at DESC, f.uid, o.uid, r.kind LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![uid, i64::from(limit)], |row| {
                let kind: String = row.get(2)?;
                let parsed = parse_rel_kind(&kind).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        format!("unknown relation kind {kind:?}").into(),
                    )
                })?;
                Ok(ThreadRelation {
                    from: row.get(0)?,
                    to: row.get(1)?,
                    kind: parsed,
                    origin: row.get(3)?,
                    at: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The threads linked to thread `uid` in either direction, as `(uid, kind, outgoing, origin)`
    /// where `outgoing` says the thread is the `from` end, at most `limit` (D144, second edge).
    pub fn thread_relations(
        &self,
        uid: &str,
        limit: u32,
    ) -> Result<Vec<(String, ThreadRelKind, bool, String)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT o.uid, r.kind, r.from_thread = t.id, r.origin
             FROM thread_rel r
             JOIN thread t ON t.uid = ?1
             JOIN thread o ON o.id = CASE WHEN r.from_thread = t.id THEN r.to_thread ELSE r.from_thread END
             WHERE r.from_thread = t.id OR r.to_thread = t.id
             ORDER BY o.uid, r.kind LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![uid, i64::from(limit)], |row| {
                let kind: String = row.get(1)?;
                let parsed = parse_rel_kind(&kind).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        format!("unknown relation kind {kind:?}").into(),
                    )
                })?;
                Ok((row.get(0)?, parsed, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn parse_rel_kind(text: &str) -> Option<ThreadRelKind> {
    match text {
        "continues" => Some(ThreadRelKind::Continues),
        "relates" => Some(ThreadRelKind::Relates),
        "supersedes" => Some(ThreadRelKind::Supersedes),
        _ => None,
    }
}

/// A label name as stored: trimmed, not empty, at most `NAME_CHARS` characters, no control
/// characters.
pub fn check_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a label needs a name"));
    }
    if name.chars().count() > NAME_CHARS {
        return Err(invalid(&format!(
            "a label name is at most {NAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a label name cannot contain control characters"));
    }
    Ok(name.to_owned())
}

/// A description as stored: trimmed, at most `DESCRIPTION_CHARS` characters.
pub fn check_description(description: &str) -> Result<String, StoreError> {
    let description = description.trim();
    if description.chars().count() > DESCRIPTION_CHARS {
        return Err(invalid(&format!(
            "a label description is at most {DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(description.to_owned())
}

/// A colour as stored: `#rrggbb` in lower case, or none.
pub fn check_color(color: Option<&str>) -> Result<Option<String>, StoreError> {
    let Some(color) = color else {
        return Ok(None);
    };
    let hex = color.strip_prefix('#');
    match hex {
        Some(hex) if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
            Ok(Some(format!("#{}", hex.to_ascii_lowercase())))
        }
        _ => Err(invalid("a label colour is #rrggbb")),
    }
}

/// The reference of an attachment, checked against its kind: a thread must exist, a path must be
/// repository-relative, a commit needs its snapshot (and is stored by its full id), and a label
/// must name another label that exists.
fn check_reference(
    tx: &Transaction<'_>,
    kind: AttachKind,
    reference: &str,
    snapshot: Option<&CommitSnapshot>,
    label: i64,
) -> Result<String, StoreError> {
    match kind {
        AttachKind::Thread => {
            thread_id(tx, reference).map_err(|error| match error {
                StoreError::NotFound => invalid("Thread not found."),
                other => other,
            })?;
            Ok(reference.to_owned())
        }
        AttachKind::File => {
            let path = reference.trim();
            if path.is_empty()
                || path.starts_with('/')
                || path.contains('\0')
                || path.split('/').any(|part| part == "..")
            {
                return Err(invalid("a file label needs a repository-relative path"));
            }
            Ok(path.to_owned())
        }
        AttachKind::Commit => {
            let snapshot = snapshot.ok_or_else(|| invalid("a commit label needs a commit"))?;
            Ok(snapshot.id.clone())
        }
        AttachKind::Label => {
            let other: i64 = reference
                .trim()
                .parse()
                .map_err(|_| invalid("a label can only attach to another label's id"))?;
            if other == label {
                return Err(invalid("a label cannot attach to itself"));
            }
            label_by_id(tx, other)?;
            Ok(other.to_string())
        }
    }
}

/// The two thread ids of a relation, or `NotFound`-as-message when a uid is unknown.
fn relation_ends(tx: &Transaction<'_>, from: &str, to: &str) -> Result<(i64, i64), StoreError> {
    let missing = |error: StoreError| match error {
        StoreError::NotFound => invalid("Thread not found."),
        other => other,
    };
    Ok((
        thread_id(tx, from).map_err(missing)?,
        thread_id(tx, to).map_err(missing)?,
    ))
}

fn relation_row(
    tx: &Transaction<'_>,
    from: i64,
    to: i64,
    kind: ThreadRelKind,
) -> Result<ThreadRelation, StoreError> {
    tx.query_row(
        "SELECT f.uid, t.uid, r.origin, r.at FROM thread_rel r
         JOIN thread f ON f.id = r.from_thread
         JOIN thread t ON t.id = r.to_thread
         WHERE r.from_thread = ?1 AND r.to_thread = ?2 AND r.kind = ?3",
        params![from, to, rel_kind_text(kind)],
        |row| {
            Ok(ThreadRelation {
                from: row.get(0)?,
                to: row.get(1)?,
                kind,
                origin: row.get(2)?,
                at: row.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or(StoreError::NotFound)
}

fn label_by_id(conn: &rusqlite::Connection, id: i64) -> Result<Label, StoreError> {
    let sql = format!("SELECT {LABEL_COLUMNS} FROM label l WHERE l.id = ?1");
    conn.query_row(&sql, params![id], label_row)
        .optional()?
        .ok_or(StoreError::NotFound)
}

fn label_by_name(conn: &rusqlite::Connection, name: &str) -> Result<Option<Label>, StoreError> {
    let sql = format!("SELECT {LABEL_COLUMNS} FROM label l WHERE l.name = ?1");
    Ok(conn.query_row(&sql, params![name], label_row).optional()?)
}

fn insert_label(
    tx: &Transaction<'_>,
    name: &str,
    description: &str,
    color: Option<&str>,
    origin: &str,
    at: i64,
) -> Result<Label, StoreError> {
    tx.execute(
        "INSERT INTO label (name, description, color, origin, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![name, description, color, origin, at],
    )?;
    label_by_id(tx, tx.last_insert_rowid())
}

fn label_row(row: &Row<'_>) -> rusqlite::Result<Label> {
    Ok(Label {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        color: row.get(3)?,
        origin: row.get(4)?,
        created_at: row.get(5)?,
        attachments: row.get(6)?,
    })
}

/// One attachment row. An unknown kind in the table is an error, not a row to skip.
fn attachment_row(row: &Row<'_>) -> rusqlite::Result<Result<LabelAttachment, StoreError>> {
    let kind_text: String = row.get(0)?;
    let Some(kind) = parse_attach_kind(&kind_text) else {
        return Ok(Err(invalid(&format!("unknown label kind {kind_text}"))));
    };
    Ok(Ok(LabelAttachment {
        kind,
        reference: row.get(1)?,
        origin: row.get(2)?,
        at: row.get(3)?,
        subject: row.get(4)?,
        author: row.get(5)?,
        committed_at: row.get(6)?,
        // The stored cache until the read resolves it (D145); the read sets `state` and
        // `current_path` (threads_session).
        resolved: row.get(7)?,
        state: None,
        current_path: None,
    }))
}

fn attach_kind_text(kind: AttachKind) -> &'static str {
    match kind {
        AttachKind::Thread => "thread",
        AttachKind::File => "file",
        AttachKind::Commit => "commit",
        AttachKind::Label => "label",
    }
}

fn parse_attach_kind(text: &str) -> Option<AttachKind> {
    match text {
        "thread" => Some(AttachKind::Thread),
        "file" => Some(AttachKind::File),
        "commit" => Some(AttachKind::Commit),
        "label" => Some(AttachKind::Label),
        _ => None,
    }
}

pub(super) fn rel_kind_text(kind: ThreadRelKind) -> &'static str {
    match kind {
        ThreadRelKind::Continues => "continues",
        ThreadRelKind::Relates => "relates",
        ThreadRelKind::Supersedes => "supersedes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::store::Batch;
    use gm_schema::threads::{LinkKind, LinkOrigin, ThreadFilter, ThreadLink};

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn thread(store: &mut ThreadStore, session: &str) -> String {
        let batch = Batch {
            harness: "claude-code".into(),
            session_id: session.into(),
            worktree: "/repo".into(),
            branch: Some("main".into()),
            head: None,
            model: None,
            harness_version: None,
            transcript: None,
            cursor: None,
            started_at: 1,
            at: 2,
            ended: None,
            entries: Vec::new(),
            gaps: Vec::new(),
        };
        store.apply(&batch).expect("apply").uid
    }

    fn snapshot() -> CommitSnapshot {
        CommitSnapshot {
            id: OID.to_owned(),
            subject: "fix the parser".to_owned(),
            author: "Ada".to_owned(),
            committed_at: 1_700_000_000_000,
        }
    }

    fn attach_thread(store: &mut ThreadStore, name: &str, uid: &str, origin: &str) -> Label {
        store
            .attach_label(name, AttachKind::Thread, uid, None, origin, 3)
            .expect("attach")
            .0
    }

    #[test]
    fn create_returns_the_label_of_the_same_name_and_keeps_its_fields() {
        let mut store = ThreadStore::open_memory().expect("store");
        let first = store
            .create_label("bug", "a defect", Some("#AABBCC"), "human", 1)
            .expect("create");
        assert_eq!(first.color.as_deref(), Some("#aabbcc"));
        let again = store
            .create_label("bug", "other", None, "agent:x", 2)
            .expect("create again");
        assert_eq!(again, first);
        assert_eq!(store.list_labels("", 10).expect("list").len(), 1);
    }

    #[test]
    fn names_colours_and_descriptions_are_checked() {
        let mut store = ThreadStore::open_memory().expect("store");
        let refused =
            |result: Result<Label, StoreError>| matches!(result, Err(StoreError::Invalid(_)));
        assert!(refused(store.create_label("  ", "", None, "human", 1)));
        assert!(refused(store.create_label(
            &"n".repeat(65),
            "",
            None,
            "human",
            1
        )));
        assert!(refused(store.create_label("a\tb", "", None, "human", 1)));
        assert!(refused(store.create_label(
            "c",
            "",
            Some("red"),
            "human",
            1
        )));
        assert!(refused(store.create_label(
            "c",
            "",
            Some("#abc"),
            "human",
            1
        )));
        assert!(refused(store.create_label(
            "c",
            "",
            Some("#gggggg"),
            "human",
            1
        )));
        assert!(refused(store.create_label(
            "c",
            &"d".repeat(501),
            None,
            "human",
            1
        )));
        let longest = store
            .create_label(
                &"n".repeat(64),
                &"d".repeat(500),
                Some("#000000"),
                "human",
                1,
            )
            .expect("the limits themselves are allowed");
        assert_eq!(longest.name.chars().count(), 64);
    }

    #[test]
    fn attaching_to_a_thread_needs_the_thread_and_is_idempotent() {
        let mut store = ThreadStore::open_memory().expect("store");
        let uid = thread(&mut store, "a");
        assert!(matches!(
            store.attach_label("bug", AttachKind::Thread, "nope", None, "human", 1),
            Err(StoreError::Invalid(message)) if message == "Thread not found."
        ));
        let (label, first) = store
            .attach_label("bug", AttachKind::Thread, &uid, None, "human", 1)
            .expect("attach");
        assert_eq!(label.attachments, 1);
        assert_eq!(first.origin, "human");
        // Attaching again by an agent returns the row there, with its first origin and time.
        let (_, again) = store
            .attach_label("bug", AttachKind::Thread, &uid, None, "agent:x", 9)
            .expect("attach again");
        assert_eq!(again, first);
        assert_eq!(
            store.read_label(label.id).expect("read").label.attachments,
            1
        );
        let filter = ThreadFilter {
            tag: Some("bug".to_owned()),
            ..ThreadFilter::default()
        };
        assert_eq!(
            store.list_threads(&filter, None, 10).expect("list").len(),
            1
        );
    }

    #[test]
    fn a_file_reference_is_a_repository_path() {
        let mut store = ThreadStore::open_memory().expect("store");
        for path in ["/abs/x.rs", "../x.rs", "src/../x.rs", "  "] {
            assert!(
                store
                    .attach_label("f", AttachKind::File, path, None, "human", 1)
                    .is_err(),
                "{path}"
            );
        }
        let (_, attachment) = store
            .attach_label("f", AttachKind::File, " src/a.rs ", None, "human", 1)
            .expect("a relative path");
        assert_eq!(attachment.reference, "src/a.rs");
    }

    #[test]
    fn a_commit_attachment_keeps_the_snapshot_and_needs_one() {
        let mut store = ThreadStore::open_memory().expect("store");
        assert!(
            store
                .attach_label("c", AttachKind::Commit, OID, None, "human", 1)
                .is_err()
        );
        let (_, attachment) = store
            .attach_label(
                "c",
                AttachKind::Commit,
                "01234567",
                Some(&snapshot()),
                "human",
                1,
            )
            .expect("attach");
        assert_eq!(attachment.reference, OID);
        assert_eq!(attachment.subject.as_deref(), Some("fix the parser"));
        assert_eq!(attachment.author.as_deref(), Some("Ada"));
        assert_eq!(attachment.committed_at, Some(1_700_000_000_000));
        assert_eq!(attachment.resolved, None);
    }

    #[test]
    fn a_label_attaches_to_another_label_that_exists() {
        let mut store = ThreadStore::open_memory().expect("store");
        let a = store.create_label("a", "", None, "human", 1).expect("a");
        let b = store.create_label("b", "", None, "human", 1).expect("b");
        let (_, attachment) = store
            .attach_label("a", AttachKind::Label, &b.id.to_string(), None, "human", 1)
            .expect("a onto b");
        assert_eq!(attachment.reference, b.id.to_string());
        assert!(
            store
                .attach_label("a", AttachKind::Label, &a.id.to_string(), None, "human", 1)
                .is_err(),
            "not onto itself"
        );
        assert!(
            store
                .attach_label("a", AttachKind::Label, "9999", None, "human", 1)
                .is_err(),
            "not a label that exists"
        );
    }

    #[test]
    fn rename_keeps_names_unique() {
        let mut store = ThreadStore::open_memory().expect("store");
        let a = store.create_label("a", "", None, "human", 1).expect("a");
        store.create_label("b", "", None, "human", 1).expect("b");
        assert!(matches!(
            store.rename_label(a.id, "b"),
            Err(StoreError::Invalid(_))
        ));
        assert_eq!(store.rename_label(a.id, "c").expect("rename").name, "c");
        assert_eq!(store.rename_label(a.id, "c").expect("same name").name, "c");
    }

    #[test]
    fn deleting_a_label_removes_its_attachments_and_those_made_to_it() {
        let mut store = ThreadStore::open_memory().expect("store");
        let uid = thread(&mut store, "a");
        let bug = attach_thread(&mut store, "bug", &uid, "human").id;
        let other = store
            .create_label("other", "", None, "human", 1)
            .expect("other");
        store
            .attach_label(
                "other",
                AttachKind::Label,
                &bug.to_string(),
                None,
                "human",
                1,
            )
            .expect("other onto bug");
        store.delete_label(bug).expect("delete");
        assert_eq!(
            store.thread_summary(&uid).expect("summary").tags,
            Vec::<String>::new()
        );
        assert_eq!(
            store.read_label(other.id).expect("other").attachments.len(),
            0
        );
        assert!(matches!(store.delete_label(bug), Err(StoreError::NotFound)));
    }

    #[test]
    fn detach_removes_one_attachment() {
        let mut store = ThreadStore::open_memory().expect("store");
        let one = thread(&mut store, "a");
        let two = thread(&mut store, "b");
        attach_thread(&mut store, "bug", &one, "human");
        attach_thread(&mut store, "bug", &two, "human");
        store
            .detach_label("bug", AttachKind::Thread, &one)
            .expect("detach");
        assert!(store.thread_summary(&one).expect("one").tags.is_empty());
        assert_eq!(store.thread_summary(&two).expect("two").tags, ["bug"]);
        assert!(matches!(
            store.detach_label("bug", AttachKind::Thread, &one),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.detach_label("missing", AttachKind::Thread, &two),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn list_matches_any_case_and_counts_attachments() {
        let mut store = ThreadStore::open_memory().expect("store");
        let uid = thread(&mut store, "a");
        store
            .create_label("Bug", "", None, "human", 1)
            .expect("Bug");
        attach_thread(&mut store, "api-bug", &uid, "human");
        let found = store.list_labels("BUG", 10).expect("list");
        let names: Vec<(&str, u32)> = found
            .iter()
            .map(|label| (label.name.as_str(), label.attachments))
            .collect();
        assert_eq!(names, [("Bug", 0), ("api-bug", 1)]);
        assert_eq!(store.list_labels("", 1).expect("limit").len(), 1);
    }

    #[test]
    fn thread_links_are_added_once_and_removed_once() {
        let mut store = ThreadStore::open_memory().expect("store");
        let a = thread(&mut store, "a");
        let b = thread(&mut store, "b");
        assert!(
            store
                .link_threads(&a, &a, ThreadRelKind::Relates, "human", 1)
                .is_err()
        );
        let first = store
            .link_threads(&a, &b, ThreadRelKind::Continues, "agent:x", 1)
            .expect("link");
        assert_eq!(first.origin, "agent:x");
        let again = store
            .link_threads(&a, &b, ThreadRelKind::Continues, "human", 2)
            .expect("again");
        assert_eq!(again, first);
        let removed = store
            .remove_thread_relation(&a, &b, ThreadRelKind::Continues)
            .expect("remove");
        assert_eq!(removed, first);
        assert!(matches!(
            store.remove_thread_relation(&a, &b, ThreadRelKind::Continues),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn deleting_a_thread_removes_its_attachments_and_links() {
        let mut store = ThreadStore::open_memory().expect("store");
        let a = thread(&mut store, "a");
        let b = thread(&mut store, "b");
        let bug = attach_thread(&mut store, "bug", &a, "human").id;
        store
            .link_threads(&a, &b, ThreadRelKind::Relates, "human", 1)
            .expect("link");
        store.delete_thread(&a).expect("delete");
        let raw = &store.conn;
        let attached: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM label_attach WHERE ref = ?1",
                params![a],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(attached, 0);
        let relations: i64 = raw
            .query_row("SELECT COUNT(*) FROM thread_rel", [], |row| row.get(0))
            .expect("count");
        assert_eq!(relations, 0);
        assert_eq!(
            store
                .read_label(bug)
                .expect("label stays")
                .attachments
                .len(),
            0
        );
    }

    #[test]
    fn commit_snapshots_fill_only_the_links_that_lack_one() {
        let mut store = ThreadStore::open_memory().expect("store");
        let uid = thread(&mut store, "a");
        store
            .link(
                &uid,
                &ThreadLink {
                    kind: LinkKind::Commit,
                    reference: OID.to_owned(),
                    origin: LinkOrigin::Observed,
                    resolved: None,
                    state: None,
                },
            )
            .expect("link");
        assert_eq!(store.commit_refs_without_snapshot().expect("refs"), [OID]);
        store.set_commit_snapshot(OID, &snapshot()).expect("fill");
        assert!(
            store
                .commit_refs_without_snapshot()
                .expect("refs")
                .is_empty()
        );
        let mut later = snapshot();
        later.subject = "rewritten".to_owned();
        store.set_commit_snapshot(OID, &later).expect("second fill");
        let subject: String = store
            .conn
            .query_row(
                "SELECT subject FROM thread_link WHERE ref = ?1",
                params![OID],
                |row| row.get(0),
            )
            .expect("subject");
        assert_eq!(subject, "fix the parser", "a snapshot is kept once taken");
    }

    #[test]
    fn check_helpers_accept_the_documented_forms() {
        assert_eq!(check_color(None).expect("none"), None);
        assert_eq!(
            check_color(Some("#0A1b2C")).expect("hex"),
            Some("#0a1b2c".to_owned())
        );
        assert_eq!(check_name("  x  ").expect("name"), "x");
        assert_eq!(
            repo_path_check("/r/a/b.rs", "/r"),
            "a/b.rs",
            "the same rule as the migration"
        );
    }

    fn repo_path_check(path: &str, worktree: &str) -> String {
        crate::threads::store::repo_path(path, worktree)
    }
}
