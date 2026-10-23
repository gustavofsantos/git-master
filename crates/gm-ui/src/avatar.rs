//! Author pictures (D110). Git stores a name and an email, never a picture, so a
//! picture is looked up on Gravatar by the SHA-256 of the email. The lookup is
//! off until the person turns it on, because it tells a third party which emails
//! appear in the repository. A picture is kept on disk under the app's config
//! directory, so it is fetched once; an email without one is remembered too.
//! Without a picture, a coloured disc with the person's initials is drawn.
//!
//! The parsing and the naming here are pure. `fetch` is the only code that does
//! I/O and runs on a background task.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The side, in pixels, asked of Gravatar: twice what is drawn, for sharp discs.
const FETCH_SIZE: u32 = 96;
/// A picture larger than this is not a picture to keep.
const MAX_BYTES: u64 = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(8);

/// Someone named in a commit: the author, or a `Co-authored-by` trailer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
}

/// The people of the `Co-authored-by:` trailers in a commit message, in order,
/// each email once. The key is read without regard to case, as Git does.
#[must_use]
pub fn co_authors(message: &str) -> Vec<Person> {
    let mut seen = HashSet::new();
    let mut people = Vec::new();
    for line in message.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("co-authored-by") {
            continue;
        }
        let value = value.trim();
        let Some((name, rest)) = value.split_once('<') else {
            continue;
        };
        let Some((email, _)) = rest.split_once('>') else {
            continue;
        };
        let (name, email) = (name.trim(), email.trim());
        if email.is_empty() || !seen.insert(email.to_lowercase()) {
            continue;
        }
        people.push(Person {
            name: if name.is_empty() { email } else { name }.to_owned(),
            email: email.to_owned(),
        });
    }
    people
}

/// The key Gravatar knows an email by: the SHA-256 of its trimmed, lowercase text.
#[must_use]
pub fn email_hash(email: &str) -> String {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    digest.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// The address of the picture. `d=404` makes Gravatar answer "not found" for an
/// email it does not know, instead of a stock image.
#[must_use]
pub fn gravatar_url(email: &str) -> String {
    format!(
        "https://gravatar.com/avatar/{}?s={FETCH_SIZE}&d=404",
        email_hash(email)
    )
}

/// Up to two capital letters for a name: the first letters of its first and last
/// words. A name with no letter or digit gives `?`.
#[must_use]
pub fn initials(name: &str) -> String {
    let mut words = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty());
    let first = words.next();
    let last = words.next_back();
    let letters: String = [first, last]
        .into_iter()
        .flatten()
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() {
        "?".to_owned()
    } else {
        letters
    }
}

/// A hue in `0.0..1.0` for the disc of an email, the same every time.
#[must_use]
pub fn hue(email: &str) -> f32 {
    let digest = Sha256::digest(email.trim().to_lowercase().as_bytes());
    f32::from(u16::from_be_bytes([digest[0], digest[1]])) / 65536.
}

fn picture_file(dir: &Path, email: &str) -> PathBuf {
    dir.join(format!("{}.img", email_hash(email)))
}

fn missing_file(dir: &Path, email: &str) -> PathBuf {
    dir.join(format!("{}.none", email_hash(email)))
}

/// What is kept on disk for an email.
#[derive(Debug, PartialEq, Eq)]
pub enum Kept {
    Picture(PathBuf),
    /// Gravatar has no picture for it.
    Absent,
}

/// What is on disk for `email`, `None` when nothing is yet.
#[must_use]
pub fn cached(dir: &Path, email: &str) -> Option<Kept> {
    let picture = picture_file(dir, email);
    if picture.is_file() {
        Some(Kept::Picture(picture))
    } else if missing_file(dir, email).is_file() {
        Some(Kept::Absent)
    } else {
        None
    }
}

/// Looks the picture up on Gravatar and keeps it in `dir`. `Some` is the file;
/// `None` is "Gravatar has none", which is kept so it is not asked again. A
/// network failure also gives `None` but keeps nothing, so a later run retries.
#[must_use]
pub fn fetch(dir: &Path, email: &str) -> Option<PathBuf> {
    match cached(dir, email) {
        Some(Kept::Picture(path)) => return Some(path),
        Some(Kept::Absent) => return None,
        None => {}
    }
    fs::create_dir_all(dir).ok()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent.get(&gravatar_url(email)).call().ok()?;
    match response.status().as_u16() {
        200 => {}
        404 => {
            let _ = fs::write(missing_file(dir, email), b"");
            return None;
        }
        _ => return None,
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let target = picture_file(dir, email);
    let temp = target.with_extension("part");
    fs::write(&temp, &bytes).ok()?;
    fs::rename(&temp, &target).ok()?;
    Some(target)
}

/// What the window knows about pictures, shared with the tasks that fetch them.
#[derive(Clone, Default)]
pub struct Avatars {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Where pictures are kept; `None` when the app has no config directory.
    dir: Option<PathBuf>,
    enabled: bool,
    /// Per lowercase email: the file, or `None` when there is no picture.
    known: HashMap<String, Option<PathBuf>>,
    asked: HashSet<String>,
}

impl Avatars {
    #[must_use]
    pub fn new(dir: Option<PathBuf>, enabled: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                dir,
                enabled,
                ..State::default()
            })),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.state().enabled = enabled;
    }

    /// The picture of `email`, when the lookup is on and one is known.
    #[must_use]
    pub fn picture(&self, email: &str) -> Option<PathBuf> {
        let state = self.state();
        if !state.enabled {
            return None;
        }
        state.known.get(&email.trim().to_lowercase())?.clone()
    }

    /// The emails among `emails` that need a lookup, marked as asked so each is
    /// asked once. Gives the directory to keep them in. Empty when the lookup is off.
    #[must_use]
    pub fn to_fetch(&self, emails: &[&str]) -> (Option<PathBuf>, Vec<String>) {
        let mut state = self.state();
        if !state.enabled {
            return (None, Vec::new());
        }
        let Some(dir) = state.dir.clone() else {
            return (None, Vec::new());
        };
        let mut wanted = Vec::new();
        for email in emails {
            let key = email.trim().to_lowercase();
            if key.is_empty() || state.known.contains_key(&key) || !state.asked.insert(key.clone())
            {
                continue;
            }
            wanted.push(key);
        }
        (Some(dir), wanted)
    }

    /// Records the answer of a lookup. A failed one is forgotten, so the next
    /// time the commit is shown it is asked again.
    pub fn record(&self, dir: &Path, email: &str, found: Option<PathBuf>) {
        let mut state = self.state();
        let absent = found.is_none() && missing_file(dir, email).is_file();
        if found.is_some() || absent {
            state.known.insert(email.to_owned(), found);
        } else {
            state.asked.remove(email);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn co_authors_come_from_the_trailers_once_each() {
        let message = "Fix it\n\nBody.\n\nCo-authored-by: Ana Lima <ana@example.test>\n\
            co-authored-by: Bo <bo@example.test>\nCo-Authored-By: Ana L. <ANA@example.test>\n\
            Signed-off-by: Cy <cy@example.test>\nCo-authored-by: no email here\n";
        assert_eq!(
            co_authors(message),
            vec![
                Person {
                    name: "Ana Lima".into(),
                    email: "ana@example.test".into()
                },
                Person {
                    name: "Bo".into(),
                    email: "bo@example.test".into()
                },
            ]
        );
    }

    #[test]
    fn a_trailer_without_a_name_is_named_by_its_email() {
        let people = co_authors("Co-authored-by: <x@example.test>");
        assert_eq!(people[0].name, "x@example.test");
    }

    #[test]
    fn the_hash_ignores_case_and_surrounding_space() {
        assert_eq!(
            email_hash(" Ana@Example.test "),
            email_hash("ana@example.test")
        );
        assert_eq!(email_hash("a").len(), 64);
        assert!(gravatar_url("a@b.test").ends_with("?s=96&d=404"));
    }

    #[test]
    fn initials_are_the_first_and_last_words() {
        assert_eq!(initials("Ana Maria de Lima"), "AL");
        assert_eq!(initials("zoë"), "Z");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("ana-lima"), "AL");
    }

    #[test]
    fn the_hue_is_stable_and_in_range() {
        assert!((0.0..1.0).contains(&hue("a@b.test")));
        assert!((hue("A@b.test") - hue("a@b.test")).abs() < f32::EPSILON);
    }

    #[test]
    fn the_cache_tells_a_picture_from_a_known_absence() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(cached(dir.path(), "a@b.test"), None);
        fs::write(missing_file(dir.path(), "a@b.test"), b"").unwrap();
        assert_eq!(cached(dir.path(), "a@b.test"), Some(Kept::Absent));
        assert_eq!(fetch(dir.path(), "a@b.test"), None);
        fs::write(picture_file(dir.path(), "c@d.test"), b"x").unwrap();
        assert_eq!(
            cached(dir.path(), "c@d.test"),
            Some(Kept::Picture(picture_file(dir.path(), "c@d.test")))
        );
    }

    #[test]
    fn each_email_is_asked_once_and_only_when_on() {
        let off = Avatars::new(Some(PathBuf::from("/x")), false);
        assert!(off.to_fetch(&["a@b.test"]).1.is_empty());
        let on = Avatars::new(Some(PathBuf::from("/x")), true);
        assert_eq!(on.to_fetch(&["A@b.test", "a@b.test"]).1, vec!["a@b.test"]);
        assert!(on.to_fetch(&["a@b.test"]).1.is_empty());
        let none = Avatars::new(None, true);
        assert!(none.to_fetch(&["a@b.test"]).1.is_empty());
    }
}
