//! The suggested message of the commit box: what a workflow or a transform offered, kept until the
//! person uses or drops it (`WF3d`), and the rule that decides whether an answer fills the box or
//! waits here. Plain values, no GPUI, so the rules have tests without a window.

/// What to do with an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Put the message in the composer: nothing the user typed is lost.
    Apply(String),
    /// Offer the message; the user chooses Replace or Discard.
    Suggest(String),
}

/// The message a workflow offered and the person has not used or dropped. One place, so a new
/// offer replaces the one waiting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestion {
    text: Option<String>,
}

impl Suggestion {
    /// The waiting message, if any.
    #[must_use]
    pub fn current(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Offers `message` in place of any one waiting.
    pub fn offer(&mut self, message: String) {
        self.text = Some(message);
    }

    /// Drops the waiting message; the box keeps its text.
    pub fn drop_suggestion(&mut self) {
        self.text = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_replaces_the_one_waiting_and_a_drop_clears_it() {
        let mut suggestion = Suggestion::default();
        assert_eq!(suggestion.current(), None);
        suggestion.offer("feat: one".to_owned());
        suggestion.offer("feat: two".to_owned());
        assert_eq!(suggestion.current(), Some("feat: two"));
        suggestion.drop_suggestion();
        assert_eq!(suggestion.current(), None);
    }
}
