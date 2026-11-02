//! Preview features (D169): work-in-progress areas that stay in the code and in the author's own
//! builds, and are hidden in the public release build. The build default and each feature's
//! `GITMASTER_ENABLE_<ID>` variable decide which are on for a process. Pure: the binary passes
//! the environment in.

/// A preview feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Branch stacks and restack (D138).
    Stacks,
    /// The Conflicts page that resolves a file's conflicts by hand.
    ConflictResolver,
}

impl Feature {
    /// Every preview feature, in the order they are listed and enabled.
    pub const ALL: [Feature; 2] = [Feature::Stacks, Feature::ConflictResolver];

    /// The name the environment variable and `--version` use.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Feature::Stacks => "stacks",
            Feature::ConflictResolver => "conflict-resolver",
        }
    }

    /// The name a menu or the documentation shows.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Feature::Stacks => "Stacks and restack",
            Feature::ConflictResolver => "Conflict resolver",
        }
    }

    /// The feature with this id, if any. The id is compared as written.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|feature| feature.id() == id)
    }

    /// The environment variable that switches this feature: `GITMASTER_ENABLE_` and the id in
    /// capitals, with `-` as `_`, such as `GITMASTER_ENABLE_CONFLICT_RESOLVER` (D169).
    #[must_use]
    pub fn env_var(self) -> String {
        format!(
            "{ENV_PREFIX}{}",
            self.id().to_ascii_uppercase().replace('-', "_")
        )
    }

    /// The bit of this feature in a `FeatureSet`: its place in `ALL`.
    fn bit(self) -> u8 {
        1 << Feature::ALL
            .iter()
            .position(|feature| *feature == self)
            .unwrap_or_default()
    }
}

/// The preview features that are on. Copy, so a window's context can carry it by value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureSet {
    bits: u8,
}

impl FeatureSet {
    /// No preview feature is on: the public build's set.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Every preview feature is on: the author's build, and the tests that predate them.
    #[must_use]
    pub fn all() -> Self {
        Self {
            bits: Feature::ALL
                .into_iter()
                .fold(0, |bits, feature| bits | feature.bit()),
        }
    }

    /// Whether `feature` is on.
    #[must_use]
    pub fn is_enabled(self, feature: Feature) -> bool {
        self.bits & feature.bit() != 0
    }

    /// The features that are on, in `Feature::ALL` order.
    pub fn enabled(self) -> impl Iterator<Item = Feature> {
        Feature::ALL
            .into_iter()
            .filter(move |feature| self.is_enabled(*feature))
    }

    fn insert(&mut self, feature: Feature) {
        self.bits |= feature.bit();
    }
}

/// The prefix of every feature's environment variable.
const ENV_PREFIX: &str = "GITMASTER_ENABLE_";

/// The set for this process, and the warnings about the variables it read (D169).
///
/// `build_default` is whether the build turns the features on (the `preview` Cargo feature).
/// `vars` are the environment's `(name, value)` pairs. Each feature reads its own variable,
/// trimmed and case-insensitive: unset or empty takes the build's default; `1`, `true`, `on` or
/// `yes` turns it on; `0`, `false`, `off` or `no` turns it off; anything else takes the build's
/// default and warns. A variable with the `GITMASTER_ENABLE_` prefix that names no feature warns.
#[must_use]
pub fn resolve(
    build_default: bool,
    vars: impl IntoIterator<Item = (String, String)>,
) -> (FeatureSet, Vec<String>) {
    let vars: Vec<(String, String)> = vars.into_iter().collect();
    let mut set = FeatureSet::none();
    let mut warnings = Vec::new();
    for feature in Feature::ALL {
        let name = feature.env_var();
        let text = vars
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.trim());
        let on = match text {
            None | Some("") => build_default,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" => true,
                "0" | "false" | "off" | "no" => false,
                _ => {
                    warnings.push(format!("{name}: \"{value}\" is not a switch; use 1 or 0"));
                    build_default
                }
            },
        };
        if on {
            set.insert(feature);
        }
    }
    for (key, _) in &vars {
        let names_feature = Feature::ALL.iter().any(|feature| feature.env_var() == *key);
        if key.starts_with(ENV_PREFIX) && !names_feature {
            warnings.push(format!(
                "unknown preview feature variable {key}; known: {}",
                known_vars()
            ));
        }
    }
    (set, warnings)
}

fn known_vars() -> String {
    Feature::ALL.map(Feature::env_var).join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_feature_has_its_id_and_title() {
        assert_eq!(Feature::Stacks.id(), "stacks");
        assert_eq!(Feature::ConflictResolver.id(), "conflict-resolver");
        assert_eq!(Feature::Stacks.title(), "Stacks and restack");
        assert_eq!(Feature::ConflictResolver.title(), "Conflict resolver");
    }

    #[test]
    fn from_id_finds_a_feature_by_its_exact_id() {
        assert_eq!(Feature::from_id("stacks"), Some(Feature::Stacks));
        assert_eq!(
            Feature::from_id("conflict-resolver"),
            Some(Feature::ConflictResolver)
        );
        assert_eq!(Feature::from_id("Stacks"), None);
        assert_eq!(Feature::from_id("conflict_resolver"), None);
        assert_eq!(Feature::from_id(""), None);
    }

    #[test]
    fn none_enables_nothing_and_all_enables_every_feature_in_order() {
        assert_eq!(FeatureSet::none().enabled().count(), 0);
        assert!(!FeatureSet::none().is_enabled(Feature::Stacks));
        assert_eq!(FeatureSet::default(), FeatureSet::none());
        assert_eq!(
            FeatureSet::all().enabled().collect::<Vec<_>>(),
            Feature::ALL.to_vec()
        );
        assert!(FeatureSet::all().is_enabled(Feature::ConflictResolver));
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn each_feature_has_its_environment_variable() {
        assert_eq!(Feature::Stacks.env_var(), "GITMASTER_ENABLE_STACKS");
        assert_eq!(
            Feature::ConflictResolver.env_var(),
            "GITMASTER_ENABLE_CONFLICT_RESOLVER"
        );
    }

    #[test]
    fn unset_or_blank_variables_take_the_build_default() {
        for pairs in [
            vec![],
            vec![("GITMASTER_ENABLE_STACKS", "")],
            vec![("GITMASTER_ENABLE_STACKS", "   ")],
        ] {
            let (on, warnings) = resolve(true, vars(&pairs));
            assert!(on.is_enabled(Feature::Stacks), "{pairs:?}");
            assert!(on.is_enabled(Feature::ConflictResolver), "{pairs:?}");
            assert!(warnings.is_empty());
            let (off, warnings) = resolve(false, vars(&pairs));
            assert_eq!(off, FeatureSet::none(), "{pairs:?}");
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn on_words_enable_one_feature_each() {
        for word in ["1", "true", "on", "yes", " ON ", "True"] {
            let (set, warnings) = resolve(false, vars(&[("GITMASTER_ENABLE_STACKS", word)]));
            assert_eq!(
                set.enabled().collect::<Vec<_>>(),
                vec![Feature::Stacks],
                "{word}"
            );
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn off_words_disable_a_feature_even_in_a_preview_build() {
        for word in ["0", "false", "off", "no", " OFF ", "No"] {
            let (set, warnings) = resolve(true, vars(&[("GITMASTER_ENABLE_STACKS", word)]));
            assert!(!set.is_enabled(Feature::Stacks), "{word}");
            assert!(set.is_enabled(Feature::ConflictResolver), "{word}");
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn each_variable_switches_only_its_own_feature() {
        let (set, warnings) = resolve(false, vars(&[("GITMASTER_ENABLE_CONFLICT_RESOLVER", "1")]));
        assert_eq!(
            set.enabled().collect::<Vec<_>>(),
            vec![Feature::ConflictResolver]
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_value_that_is_not_a_switch_takes_the_build_default_and_warns() {
        let (on, warnings) = resolve(true, vars(&[("GITMASTER_ENABLE_STACKS", "maybe")]));
        assert!(on.is_enabled(Feature::Stacks));
        assert_eq!(
            warnings,
            vec!["GITMASTER_ENABLE_STACKS: \"maybe\" is not a switch; use 1 or 0".to_owned()]
        );
        let (off, warnings) = resolve(false, vars(&[("GITMASTER_ENABLE_STACKS", "2")]));
        assert_eq!(off, FeatureSet::none());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn an_unknown_enable_variable_warns_with_the_known_ones() {
        let (set, warnings) = resolve(false, vars(&[("GITMASTER_ENABLE_FOO", "1")]));
        assert_eq!(set, FeatureSet::none());
        assert_eq!(
            warnings,
            vec![
                "unknown preview feature variable GITMASTER_ENABLE_FOO; known: GITMASTER_ENABLE_STACKS, GITMASTER_ENABLE_CONFLICT_RESOLVER"
                    .to_owned()
            ]
        );
    }

    #[test]
    fn variables_outside_the_prefix_are_ignored() {
        let (set, warnings) = resolve(false, vars(&[("HOME", "/x"), ("GITMASTER_THEME", "dark")]));
        assert_eq!(set, FeatureSet::none());
        assert!(warnings.is_empty());
    }
}
