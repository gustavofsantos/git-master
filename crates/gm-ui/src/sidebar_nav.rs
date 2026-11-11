//! The sidebar's navigation (UI redesign, slice 1): the six pages at the top, the footer
//! entries, and which page a screen belongs to. Pure: `shell_sidebar` draws what this lists.
//!
//! A page groups the screens that used to be separate sidebar entries: Code holds the Code
//! viewer and the text search, Agents holds the threads and the labels, Changes holds the
//! Working Copy and the conflict resolver.

use crate::action_registry::Action;
use crate::view::Screen;

/// A page of the sidebar's first group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Inbox,
    Changes,
    Review,
    History,
    Code,
    Agents,
}

impl Page {
    /// The pages, top to bottom.
    pub(crate) const ALL: [Page; 6] = [
        Page::Inbox,
        Page::Changes,
        Page::Review,
        Page::History,
        Page::Code,
        Page::Agents,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Page::Inbox => "Inbox",
            Page::Changes => "Changes",
            Page::Review => "Review",
            Page::History => "History",
            Page::Code => "Code",
            Page::Agents => "Agents",
        }
    }

    /// The registry action a click on the page runs. Changes has none: the window shows the
    /// Working Copy itself, as the old Working Copy entry did.
    pub(crate) fn action(self) -> Option<Action> {
        match self {
            Page::Inbox => Some(Action::ShowInbox),
            Page::Changes => None,
            Page::Review => Some(Action::ShowLocalReview),
            Page::History => Some(Action::ShowHistory),
            Page::Code => Some(Action::ShowCode),
            Page::Agents => Some(Action::ShowThreads),
        }
    }

    /// The page `screen` belongs to; `None` for the screens of the footer.
    pub(crate) fn of(screen: Screen) -> Option<Page> {
        match screen {
            Screen::Inbox => Some(Page::Inbox),
            Screen::WorkingCopy | Screen::Conflicts => Some(Page::Changes),
            Screen::LocalReview => Some(Page::Review),
            Screen::History => Some(Page::History),
            Screen::Code | Screen::Search | Screen::Health => Some(Page::Code),
            Screen::Threads | Screen::Labels => Some(Page::Agents),
            Screen::Workflows | Screen::Stashes | Screen::Reflog => None,
        }
    }
}

/// An entry of the sidebar's footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Footer {
    Stashes,
    Reflog,
    Workflows,
    Settings,
}

impl Footer {
    pub(crate) const ALL: [Footer; 4] = [
        Footer::Stashes,
        Footer::Reflog,
        Footer::Workflows,
        Footer::Settings,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Footer::Stashes => "Stashes",
            Footer::Reflog => "Reflog",
            Footer::Workflows => "Workflows",
            Footer::Settings => "Settings",
        }
    }

    pub(crate) fn action(self) -> Action {
        match self {
            Footer::Stashes => Action::ShowStashes,
            Footer::Reflog => Action::ShowReflog,
            Footer::Workflows => Action::ShowWorkflows,
            Footer::Settings => Action::OpenSettings,
        }
    }

    /// The footer entry `screen` is, if any.
    pub(crate) fn of(screen: Screen) -> Option<Footer> {
        match screen {
            Screen::Stashes => Some(Footer::Stashes),
            Screen::Reflog => Some(Footer::Reflog),
            Screen::Workflows => Some(Footer::Workflows),
            _ => None,
        }
    }
}

/// The badge a page shows, from the counts the window knows. `None` hides it: a page shows
/// a badge only when its count is known, and Review and Inbox only when it is not zero.
#[must_use]
pub(crate) fn badge(page: Page, counts: &Counts) -> Option<String> {
    match page {
        Page::Inbox => counts.inbox.filter(|n| *n > 0).map(|n| n.to_string()),
        Page::Changes => Some(counts.changed.to_string()),
        Page::Review => counts
            .open_comments
            .filter(|n| *n > 0)
            .map(|n| n.to_string()),
        Page::History | Page::Code => None,
        Page::Agents => counts.threads.map(|n| n.to_string()),
    }
}

/// The counts behind the badges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) inbox: Option<usize>,
    pub(crate) changed: usize,
    pub(crate) open_comments: Option<usize>,
    pub(crate) threads: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENS: [Screen; 13] = [
        Screen::Inbox,
        Screen::WorkingCopy,
        Screen::History,
        Screen::Workflows,
        Screen::Code,
        Screen::Search,
        Screen::Health,
        Screen::Stashes,
        Screen::Reflog,
        Screen::Threads,
        Screen::Labels,
        Screen::LocalReview,
        Screen::Conflicts,
    ];

    #[test]
    fn every_screen_belongs_to_exactly_one_page_or_footer_entry() {
        for screen in SCREENS {
            let page = Page::of(screen).is_some();
            let footer = Footer::of(screen).is_some();
            assert!(page ^ footer, "{screen:?}");
        }
    }

    #[test]
    fn the_pages_are_the_briefs_six_in_order() {
        let labels: Vec<_> = Page::ALL.iter().map(|page| page.label()).collect();
        assert_eq!(
            labels,
            ["Inbox", "Changes", "Review", "History", "Code", "Agents"]
        );
        let footer: Vec<_> = Footer::ALL.iter().map(|entry| entry.label()).collect();
        assert_eq!(footer, ["Stashes", "Reflog", "Workflows", "Settings"]);
    }

    #[test]
    fn search_is_part_of_code_and_labels_of_agents() {
        assert_eq!(Page::of(Screen::Search), Some(Page::Code));
        assert_eq!(Page::of(Screen::Health), Some(Page::Code));
        assert_eq!(Page::of(Screen::Labels), Some(Page::Agents));
        assert_eq!(Page::of(Screen::Conflicts), Some(Page::Changes));
    }

    #[test]
    fn badges_show_known_counts_and_hide_empty_review_and_inbox() {
        let counts = Counts {
            inbox: Some(0),
            changed: 3,
            open_comments: Some(0),
            threads: None,
        };
        assert_eq!(badge(Page::Changes, &counts), Some("3".to_owned()));
        assert_eq!(badge(Page::Inbox, &counts), None);
        assert_eq!(badge(Page::Review, &counts), None);
        assert_eq!(badge(Page::Agents, &counts), None);
        let counts = Counts {
            inbox: Some(2),
            open_comments: Some(4),
            threads: Some(9),
            ..counts
        };
        assert_eq!(badge(Page::Inbox, &counts), Some("2".to_owned()));
        assert_eq!(badge(Page::Review, &counts), Some("4".to_owned()));
        assert_eq!(badge(Page::Agents, &counts), Some("9".to_owned()));
        assert_eq!(badge(Page::History, &counts), None);
    }
}
