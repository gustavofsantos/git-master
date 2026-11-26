//! The tips the banner shows when the companion starts (D267). A tip names actions, not
//! keys: `{name}` stands for the keys of action `name` in the person's own key map, so a
//! remapped key shows as remapped and a tip whose action has no key is not shown.

use crate::nav::keymap::KeyMap;
use crate::nav::keys::show_sequence;

/// Things the companion does that are easy to miss.
pub const TIPS: &[&str] = &[
    "{blame.open} on a line shows who last changed it; {open} on a run's heading opens that commit.",
    "{origin.open} on a line or a selection shows how it got there: its commits and the agent threads behind them.",
    "{file.find} finds a file by its name; end the query with :120 to open it at line 120.",
    "{select.toggle} selects lines, and {review.comment} starts a review thread on them.",
    "{copy.reference} copies a reference to the lines, ready for your agent's prompt.",
    "{go.threads} lists the review threads by who acts next.",
    "{go.branch} shows the branch's changes against its base.",
    "{diff.layout} shows the diffs side by side: the old lines on the left, the new on the right.",
    "{view.wrap} switches between wrapping long lines and cutting them at the edge.",
    "{help.keys} lists every key; {search.start} searches the screen.",
    "The mouse wheel scrolls any screen, and the bar at the right edge can be dragged.",
    "No more tips: git config --global gitmaster.tips false",
];

/// `tip` with each `{action}` replaced by its shortest keys in `map`; `None` when an action it
/// names has no key there.
pub fn render(tip: &str, map: &KeyMap) -> Option<String> {
    let mut out = String::with_capacity(tip.len());
    let mut rest = tip;
    while let Some(open) = rest.find('{') {
        let close = open + rest[open..].find('}')?;
        out.push_str(&rest[..open]);
        let keys = map.keys_of(&rest[open + 1..close]);
        out.push_str(&show_sequence(keys.first()?));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The tip to show for `seed` (any number, such as the time): the first that renders, going
/// round from `seed`, so a person sees them all over time.
pub fn pick(seed: u64, map: &KeyMap) -> Option<String> {
    let count = TIPS.len();
    let start = usize::try_from(seed % count as u64).unwrap_or(0);
    (0..count).find_map(|step| render(TIPS[(start + step) % count], map))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::Mode;
    use crate::nav::defaults;

    fn normal() -> KeyMap {
        defaults::input()
            .map(Mode::Normal)
            .cloned()
            .expect("normal keys")
    }

    #[test]
    fn a_tip_names_the_keys_of_its_actions() {
        assert_eq!(
            render(TIPS[0], &normal()).as_deref(),
            Some(
                "g b on a line shows who last changed it; Enter on a run's heading opens that commit."
            )
        );
        assert_eq!(
            render("{origin.open} explains.", &normal()).as_deref(),
            Some("Space w explains.")
        );
    }

    #[test]
    fn a_tip_for_an_action_without_a_key_is_skipped() {
        assert_eq!(render("{no.such.action} here", &normal()), None);
        assert_eq!(render("{unclosed", &normal()), None);
    }

    #[test]
    fn every_tip_renders_with_the_default_keys_and_a_seed_goes_round() {
        let map = normal();
        for tip in TIPS {
            assert!(render(tip, &map).is_some(), "{tip}");
        }
        assert_eq!(pick(0, &map), render(TIPS[0], &map));
        assert_eq!(pick(TIPS.len() as u64 + 1, &map), render(TIPS[1], &map));
    }
}
