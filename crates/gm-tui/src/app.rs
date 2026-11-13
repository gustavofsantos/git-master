//! The model of the terminal companion (D235): what the screen shows and how keys and clicks
//! change it. Nothing here draws or touches the terminal, so every rule is tested on its own.
//! Each change the person asks of the repository leaves here as a `CommandKind` for the engine.

use gm_schema::{Change, CommandKind, Event, RepoStatus, StatusEntry};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

/// Narrower than this, one pane shows at a time (D235 (9)).
pub const SPLIT_MIN_WIDTH: u16 = 100;
/// Smaller than this, only a message shows (D235 (9)).
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// Rows a turn of the mouse wheel scrolls.
const WHEEL_ROWS: usize = 3;

/// The two groups of the file list, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Unstaged,
    Staged,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Unstaged => "Unstaged",
            Group::Staged => "Staged",
        }
    }
}

/// One file under one group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    pub group: Group,
    pub path: String,
    /// Git's status letter on this side: `M`, `A`, `D`, `R`, `C`, `T`, `?` for an untracked
    /// file and `U` for a conflicted one.
    pub letter: char,
}

/// A line of the file list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListRow {
    /// A group title with the number of files under it.
    Header(Group, usize),
    File(FileRow),
}

/// The pane that has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Files,
    Diff,
}

/// Where each part of the screen is, as the last draw placed it. Clicks are read against it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Areas {
    /// `None` when the terminal is too small, or when one pane shows and it is the diff.
    pub files: Option<Rect>,
    /// `None` when the terminal is too small, or when one pane shows and it is the list.
    pub diff: Option<Rect>,
    /// `None` when the terminal is too small.
    pub status: Option<Rect>,
}

/// The parts of a terminal of `size` for `focus`: the list on the left and the diff on the
/// right, one of them alone below `SPLIT_MIN_WIDTH`, nothing below the minimum size.
pub fn areas(size: Rect, focus: Focus) -> Areas {
    if size.width < MIN_WIDTH || size.height < MIN_HEIGHT {
        return Areas::default();
    }
    let body = Rect {
        height: size.height - 1,
        ..size
    };
    let status = Rect {
        y: size.y + size.height - 1,
        height: 1,
        ..size
    };
    if size.width < SPLIT_MIN_WIDTH {
        let (files, diff) = match focus {
            Focus::Files => (Some(body), None),
            Focus::Diff => (None, Some(body)),
        };
        return Areas {
            files,
            diff,
            status: Some(status),
        };
    }
    let list_width = (size.width * 2 / 5).clamp(30, 60);
    let files = Rect {
        width: list_width,
        ..body
    };
    let diff = Rect {
        x: body.x + list_width,
        width: body.width - list_width,
        ..body
    };
    Areas {
        files: Some(files),
        diff: Some(diff),
        status: Some(status),
    }
}

/// The file list of `status`: the unstaged files, then the staged ones, each group under its
/// title. A group with no file is left out. A file changed on both sides is in both groups.
pub fn list_rows(status: &RepoStatus) -> Vec<ListRow> {
    let mut unstaged = Vec::new();
    let mut staged = Vec::new();
    for entry in &status.entries {
        if let Some(letter) = unstaged_letter(entry) {
            unstaged.push(file(Group::Unstaged, entry, letter));
        }
        if let Some(change) = entry.staged.filter(|_| !entry.conflicted) {
            staged.push(file(Group::Staged, entry, change_letter(change)));
        }
    }
    let mut rows = Vec::with_capacity(unstaged.len() + staged.len() + 2);
    for (group, files) in [(Group::Unstaged, unstaged), (Group::Staged, staged)] {
        if !files.is_empty() {
            rows.push(ListRow::Header(group, files.len()));
            rows.extend(files.into_iter().map(ListRow::File));
        }
    }
    rows
}

fn file(group: Group, entry: &StatusEntry, letter: char) -> FileRow {
    FileRow {
        group,
        path: entry.path.clone(),
        letter,
    }
}

/// The letter of the unstaged side of `entry`, `None` when it has no unstaged change. A
/// conflicted file is listed only here, as `U`.
fn unstaged_letter(entry: &StatusEntry) -> Option<char> {
    if entry.conflicted {
        Some('U')
    } else if entry.untracked {
        Some('?')
    } else {
        entry.unstaged.map(change_letter)
    }
}

pub fn change_letter(change: Change) -> char {
    match change {
        Change::Added => 'A',
        Change::Modified => 'M',
        Change::Deleted => 'D',
        Change::Renamed => 'R',
        Change::Copied => 'C',
        Change::TypeChanged => 'T',
    }
}

/// The whole state of the companion.
#[derive(Debug)]
pub struct App {
    pub status: RepoStatus,
    pub rows: Vec<ListRow>,
    /// Index into `rows` of the selected file; always a `File` row, or 0 with no file.
    pub cursor: usize,
    /// The first row of the list on screen.
    pub scroll: usize,
    /// The next draw brings the cursor on screen. The wheel clears it, so the list scrolls
    /// away from the cursor until the cursor moves again.
    follow_cursor: bool,
    pub focus: Focus,
    /// Where the last draw put each part.
    pub areas: Areas,
    /// A message for the status bar, such as why watching is off.
    pub notice: Option<String>,
    pub quit: bool,
    /// What changed since the last draw.
    pub dirty: bool,
    commands: Vec<CommandKind>,
}

impl App {
    /// The companion for the repository whose status is `status`, with its first file
    /// selected.
    pub fn new(status: RepoStatus) -> Self {
        let rows = list_rows(&status);
        let mut app = Self {
            status,
            rows,
            cursor: 0,
            scroll: 0,
            follow_cursor: true,
            focus: Focus::Files,
            areas: Areas::default(),
            notice: None,
            quit: false,
            dirty: true,
            commands: Vec::new(),
        };
        app.cursor = app.first_file().unwrap_or(0);
        app
    }

    /// The selected file, if the list has any.
    pub fn selected(&self) -> Option<&FileRow> {
        match self.rows.get(self.cursor) {
            Some(ListRow::File(file)) => Some(file),
            _ => None,
        }
    }

    /// The number of files listed, a file on both sides counted once per side.
    pub fn file_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, ListRow::File(_)))
            .count()
    }

    /// The commands the model asked for since the last call, oldest first.
    pub fn take_commands(&mut self) -> Vec<CommandKind> {
        std::mem::take(&mut self.commands)
    }

    /// Takes in what the engine reports. A new status keeps the selected file when it is
    /// still listed in the same group, and otherwise keeps the cursor near where it was.
    pub fn apply(&mut self, event: &Event) {
        if let Event::StatusLoaded { status, .. } = event {
            let kept = self.selected().map(|file| (file.group, file.path.clone()));
            self.status = status.clone();
            self.rows = list_rows(&self.status);
            let same = kept.and_then(|(group, path)| {
                self.rows.iter().position(|row| {
                    matches!(row, ListRow::File(file) if file.group == group && file.path == path)
                })
            });
            self.cursor = match same {
                Some(index) => index,
                None => self.nearest_file(self.cursor).unwrap_or(0),
            };
            self.dirty = true;
        }
    }

    /// The terminal window gained or lost the person's attention, or changed size.
    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    pub fn set_notice(&mut self, notice: Option<String>) {
        self.notice = notice;
        self.dirty = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') if !ctrl => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            KeyCode::Char('j') | KeyCode::Down => self.move_files(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_files(-1),
            KeyCode::Char('d') if ctrl => self.move_files(self.half_page()),
            KeyCode::Char('u') if ctrl => self.move_files(-self.half_page()),
            KeyCode::Char('g') | KeyCode::Home => self.select(self.first_file()),
            KeyCode::Char('G') | KeyCode::End => self.select(self.last_file()),
            _ => return,
        }
        self.follow_cursor = true;
        self.dirty = true;
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        let Some(files) = self.areas.files.filter(|area| area.contains(at)) else {
            if self.areas.diff.is_some_and(|area| area.contains(at))
                && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            {
                self.focus = Focus::Diff;
                self.dirty = true;
            }
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Files;
                let index = self.scroll + usize::from(at.y - files.y);
                if matches!(self.rows.get(index), Some(ListRow::File(_))) {
                    self.cursor = index;
                }
            }
            MouseEventKind::ScrollDown => {
                let last = self.rows.len().saturating_sub(usize::from(files.height));
                self.scroll = (self.scroll + WHEEL_ROWS).min(last);
                self.follow_cursor = false;
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(WHEEL_ROWS);
                self.follow_cursor = false;
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// Moves `scroll` so the cursor is on screen in a list `height` rows high. Called by the
    /// draw, which knows the height; the wheel moves `scroll` without moving the cursor.
    pub fn keep_cursor_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        let last = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.min(last);
        if !self.follow_cursor {
            return;
        }
        if self.cursor < self.scroll {
            // Keep a group title in view above its first file.
            self.scroll = self.cursor.saturating_sub(1);
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Files => Focus::Diff,
            Focus::Diff => Focus::Files,
        };
    }

    fn half_page(&self) -> isize {
        let height = self.areas.files.map_or(MIN_HEIGHT, |area| area.height);
        isize::from(i16::try_from(height / 2).unwrap_or(i16::MAX)).max(1)
    }

    /// Moves the cursor `by` files, past the group titles, and stops at either end.
    fn move_files(&mut self, by: isize) {
        let files: Vec<usize> = self.file_indexes().collect();
        let Some(at) = files.iter().position(|&index| index == self.cursor) else {
            self.select(files.first().copied());
            return;
        };
        let target = at.saturating_add_signed(by).min(files.len() - 1);
        self.cursor = files[target];
    }

    fn select(&mut self, index: Option<usize>) {
        if let Some(index) = index {
            self.cursor = index;
        }
    }

    fn file_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, ListRow::File(_)))
            .map(|(index, _)| index)
    }

    fn first_file(&self) -> Option<usize> {
        self.file_indexes().next()
    }

    fn last_file(&self) -> Option<usize> {
        self.file_indexes().last()
    }

    /// The file row nearest `index`, looking down first.
    fn nearest_file(&self, index: usize) -> Option<usize> {
        let index = index.min(self.rows.len().saturating_sub(1));
        self.file_indexes()
            .find(|&at| at >= index)
            .or_else(|| self.last_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gm_schema::{BranchInfo, OperationId};
    use ratatui::crossterm::event::KeyEventState;

    fn entry(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> StatusEntry {
        StatusEntry {
            path: path.to_owned(),
            orig_path: None,
            staged,
            unstaged,
            untracked: false,
            conflicted: false,
            submodule: None,
        }
    }

    fn status(entries: Vec<StatusEntry>) -> RepoStatus {
        RepoStatus {
            root: "/repo".to_owned(),
            branch: BranchInfo {
                head: Some("main".to_owned()),
                ..BranchInfo::default()
            },
            entries,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn sample() -> App {
        let mut untracked = entry("new.txt", None, None);
        untracked.untracked = true;
        App::new(status(vec![
            entry("a.rs", None, Some(Change::Modified)),
            entry("b.rs", Some(Change::Added), Some(Change::Modified)),
            entry("c.rs", Some(Change::Deleted), None),
            untracked,
        ]))
    }

    fn selected_path(app: &App) -> Option<(Group, &str)> {
        app.selected().map(|file| (file.group, file.path.as_str()))
    }

    #[test]
    fn files_are_grouped_unstaged_first_and_a_file_on_both_sides_is_in_both() {
        let app = sample();
        let shown: Vec<String> = app
            .rows
            .iter()
            .map(|row| match row {
                ListRow::Header(group, count) => format!("# {} {count}", group.title()),
                ListRow::File(file) => format!("{} {}", file.letter, file.path),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "# Unstaged 3",
                "M a.rs",
                "M b.rs",
                "? new.txt",
                "# Staged 2",
                "A b.rs",
                "D c.rs"
            ]
        );
        assert_eq!(app.file_count(), 5);
    }

    #[test]
    fn a_conflicted_file_is_listed_once_as_unstaged_u() {
        let mut conflicted = entry("x.rs", Some(Change::Modified), Some(Change::Modified));
        conflicted.conflicted = true;
        let rows = list_rows(&status(vec![conflicted]));
        assert_eq!(
            rows,
            [
                ListRow::Header(Group::Unstaged, 1),
                ListRow::File(FileRow {
                    group: Group::Unstaged,
                    path: "x.rs".to_owned(),
                    letter: 'U'
                })
            ]
        );
    }

    #[test]
    fn a_clean_tree_has_no_rows_and_no_selection() {
        let app = App::new(status(Vec::new()));
        assert!(app.rows.is_empty());
        assert_eq!(app.selected(), None);
    }

    #[test]
    fn the_cursor_starts_on_the_first_file_and_skips_group_titles() {
        let mut app = sample();
        assert_eq!(selected_path(&app), Some((Group::Unstaged, "a.rs")));
        for _ in 0..3 {
            app.on_key(key(KeyCode::Char('j')));
        }
        assert_eq!(selected_path(&app), Some((Group::Staged, "b.rs")));
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(selected_path(&app), Some((Group::Unstaged, "new.txt")));
    }

    #[test]
    fn the_cursor_stops_at_either_end() {
        let mut app = sample();
        app.on_key(key(KeyCode::Up));
        assert_eq!(selected_path(&app), Some((Group::Unstaged, "a.rs")));
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(selected_path(&app), Some((Group::Staged, "c.rs")));
        app.on_key(key(KeyCode::Down));
        assert_eq!(selected_path(&app), Some((Group::Staged, "c.rs")));
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(selected_path(&app), Some((Group::Unstaged, "a.rs")));
    }

    #[test]
    fn a_new_status_keeps_the_selected_file_in_its_group() {
        let mut app = sample();
        app.on_key(key(KeyCode::Char('G')));
        // a.rs is gone; c.rs is still staged, now below fewer rows.
        let mut untracked = entry("new.txt", None, None);
        untracked.untracked = true;
        app.apply(&Event::StatusLoaded {
            id: OperationId(9),
            status: status(vec![
                entry("b.rs", Some(Change::Added), Some(Change::Modified)),
                entry("c.rs", Some(Change::Deleted), None),
                untracked,
            ]),
        });
        assert_eq!(selected_path(&app), Some((Group::Staged, "c.rs")));
    }

    #[test]
    fn when_the_selected_file_leaves_its_group_the_cursor_stays_near() {
        let mut app = sample();
        app.on_key(key(KeyCode::Down)); // b.rs, unstaged
        app.apply(&Event::StatusLoaded {
            id: OperationId(9),
            status: status(vec![
                entry("a.rs", None, Some(Change::Modified)),
                entry("b.rs", Some(Change::Modified), None),
            ]),
        });
        assert_eq!(selected_path(&app), Some((Group::Staged, "b.rs")));
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = sample();
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit);
        let mut app = sample();
        app.on_key(KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(KeyCode::Char('c'))
        });
        assert!(app.quit);
    }

    #[test]
    fn tab_switches_the_focus() {
        let mut app = sample();
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Diff);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Files);
    }

    #[test]
    fn the_terminal_is_split_from_100_columns_and_too_small_below_80_by_24() {
        let wide = areas(Rect::new(0, 0, 120, 40), Focus::Files);
        assert_eq!(wide.files, Some(Rect::new(0, 0, 48, 39)));
        assert_eq!(wide.diff, Some(Rect::new(48, 0, 72, 39)));
        assert_eq!(wide.status, Some(Rect::new(0, 39, 120, 1)));

        let narrow = areas(Rect::new(0, 0, 90, 30), Focus::Diff);
        assert_eq!(narrow.files, None);
        assert_eq!(narrow.diff, Some(Rect::new(0, 0, 90, 29)));

        assert_eq!(
            areas(Rect::new(0, 0, 79, 40), Focus::Files),
            Areas::default()
        );
        assert_eq!(
            areas(Rect::new(0, 0, 120, 23), Focus::Files),
            Areas::default()
        );
    }

    #[test]
    fn a_click_on_a_file_selects_it_and_a_click_on_a_title_does_not() {
        let mut app = sample();
        app.areas = areas(Rect::new(0, 0, 120, 40), Focus::Files);
        app.on_mouse(click(5, 6)); // row 6: "D c.rs"
        assert_eq!(selected_path(&app), Some((Group::Staged, "c.rs")));
        app.on_mouse(click(5, 4)); // row 4: the Staged title
        assert_eq!(selected_path(&app), Some((Group::Staged, "c.rs")));
        app.on_mouse(click(60, 4)); // the diff
        assert_eq!(app.focus, Focus::Diff);
        app.on_mouse(click(5, 1));
        assert_eq!(app.focus, Focus::Files);
        assert_eq!(selected_path(&app), Some((Group::Unstaged, "a.rs")));
    }

    #[test]
    fn the_cursor_is_kept_on_screen() {
        let entries = (0..50)
            .map(|n| entry(&format!("f{n:02}.rs"), None, Some(Change::Modified)))
            .collect();
        let mut app = App::new(status(entries));
        app.on_key(key(KeyCode::Char('G')));
        app.keep_cursor_visible(10);
        assert_eq!(app.scroll, 41);
        app.on_key(key(KeyCode::Char('g')));
        app.keep_cursor_visible(10);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn the_wheel_scrolls_away_from_the_cursor_until_the_cursor_moves() {
        let entries = (0..50)
            .map(|n| entry(&format!("f{n:02}.rs"), None, Some(Change::Modified)))
            .collect();
        let mut app = App::new(status(entries));
        app.areas = areas(Rect::new(0, 0, 120, 40), Focus::Files);
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            ..click(5, 5)
        };
        app.on_mouse(wheel);
        app.on_mouse(wheel);
        app.keep_cursor_visible(39);
        assert_eq!(app.scroll, 6);
        assert_eq!(app.cursor, 1);
        app.on_key(key(KeyCode::Down));
        app.keep_cursor_visible(39);
        assert_eq!(app.scroll, 1);
    }
}
