//! The action registry (D241 (3), D242 (7)): every action the companion can run, by a
//! stable name, with what the menu, the keys screen and a future palette show of it, and
//! where it applies. An action never depends on the key or click that names it: it finds
//! its target in the context.

use super::Mode;

/// Whether the menu stays open after the action, in plain words (not Hydra's colors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behavior {
    /// Do one thing, then close the menu.
    OneShot,
    /// Stay open for the next one, such as staging hunk after hunk.
    Repeat,
}

/// One action as the registry knows it.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Stable name, used by key maps and configuration: `stage`, `cursor.down`.
    pub name: &'static str,
    /// Short title for the menu: `stage hunk`.
    pub title: &'static str,
    /// One line for the keys screen and a palette.
    pub description: &'static str,
    /// The menu group: `Stage`, `Review`, `Go to`, `Move`.
    pub group: &'static str,
    pub modes: &'static [Mode],
    pub behavior: Behavior,
}

/// The registry: the specs, in the order the menu lists them in a group.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    specs: Vec<Spec>,
}

impl Registry {
    pub fn new(specs: Vec<Spec>) -> Self {
        Self { specs }
    }

    pub fn get(&self, name: &str) -> Option<&Spec> {
        self.specs.iter().find(|spec| spec.name == name)
    }

    pub fn all(&self) -> &[Spec] {
        &self.specs
    }

    /// The actions whose title or description holds every word of `query`, ignoring case,
    /// in registry order: what a palette lists.
    pub fn search(&self, query: &str) -> Vec<&Spec> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.specs
            .iter()
            .filter(|spec| {
                let text =
                    format!("{} {} {}", spec.title, spec.description, spec.name).to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Mode] = &[Mode::Normal, Mode::Visual];

    fn spec(name: &'static str, title: &'static str, description: &'static str) -> Spec {
        Spec {
            name,
            title,
            description,
            group: "Stage",
            modes: ALL,
            behavior: Behavior::OneShot,
        }
    }

    #[test]
    fn actions_are_found_by_name_and_searched_by_their_words() {
        let registry = Registry::new(vec![
            spec(
                "stage",
                "stage",
                "Stage what is under the cursor or selected.",
            ),
            spec(
                "thread.resolve",
                "resolve thread",
                "Resolve the review thread at the cursor.",
            ),
        ]);
        assert_eq!(registry.get("stage").map(|s| s.title), Some("stage"));
        assert!(registry.get("nope").is_none());
        let found: Vec<&str> = registry
            .search("Thread RES")
            .iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(found, ["thread.resolve"]);
        assert_eq!(registry.search("").len(), 2);
    }
}
