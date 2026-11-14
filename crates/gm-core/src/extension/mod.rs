//! Extensions (design/extensions.md, D253, D254): Lua scripts that claim slots of a view and
//! answer each with a typed document (`gm_schema::ext`). They run in the workflow sandbox
//! (`script::luau`) with a read-only `ctx`: `exec` of a listed program, `http.get` to a listed
//! host, `log` and `now`. Nothing an extension calls changes the repository.

pub mod cli;
pub mod focus;
pub mod host;
pub mod http;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::script::catalog::{FieldKind, FieldSpec, Level};
use crate::script::port::{ScriptDiagnostic, ScriptEngine};
use crate::script::surface::{ArgShape, CtxFn, CtxSurface};

/// The slots this build exposes.
pub const SLOTS: &[&str] = &["commit"];

/// The largest extension file.
const MAX_FILE: u64 = 256 * 1024;

/// The `ui` helpers every extension sees. They only build plain tables; the host checks them
/// (`gm_schema::ext::Document::parse`), so a hand-written table is as good as a helper's.
pub const PRELUDE: &str = r#"
local function node(t) return t end
ui = {
  section = function(title, children, opts)
    return node({ type = "section", title = title, children = children or {},
                  folded = (opts ~= nil and opts.folded) or false })
  end,
  field = function(label, value) return node({ type = "field", label = label, value = value }) end,
  text = function(inlines) return node({ type = "text", inlines = inlines }) end,
  list = function(items) return node({ type = "list", items = items }) end,
  span = function(text, tone) return node({ type = "span", text = text, tone = tone }) end,
  badge = function(text, tone) return node({ type = "badge", text = text, tone = tone }) end,
  code = function(text) return node({ type = "code", text = text }) end,
  link = function(text, target) return node({ type = "link", text = text, target = target }) end,
}
"#;

/// What `M.meta` declares (docs/reference/extension-api.md).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub name: String,
    pub description: String,
    pub api: u64,
    /// Programs `ctx.exec` may run, by name.
    pub exec: Vec<String>,
    /// Hosts `ctx.http.get` may reach.
    pub http: Vec<String>,
    /// Hosts whose Git credential a request may use; each is also in `http`.
    pub credentials: Vec<String>,
}

/// One extension file, read but not run.
#[derive(Clone, Debug, PartialEq)]
pub struct Extension {
    pub path: PathBuf,
    /// The file name, used as the chunk name in errors.
    pub file_name: String,
    /// sha256 of the text: what a review approves (D101).
    pub hash: String,
    pub source: String,
    pub meta: Meta,
    /// The slots it claims, sorted.
    pub slots: Vec<String>,
}

/// `$XDG_CONFIG_HOME/gitmaster/extensions`, or `~/.config/gitmaster/extensions`.
pub fn user_dir(
    xdg_config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    crate::paths::config_dir_under(xdg_config_home, home).map(|dir| dir.join("extensions"))
}

/// `user_dir` with this process's environment.
pub fn default_user_dir() -> Option<PathBuf> {
    user_dir(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The `ctx` of an extension: read-only calls, each one a `Read` level function.
#[must_use]
pub fn surface() -> CtxSurface {
    static EXEC: &[FieldSpec] = &[
        field(
            "argv",
            FieldKind::TextList,
            true,
            "The program and its arguments.",
        ),
        field("stdin", FieldKind::Text, false, "Text for standard input."),
        field(
            "timeout",
            FieldKind::Duration,
            false,
            "Such as 10s. Default 10s.",
        ),
        field(
            "format",
            FieldKind::Choice(&["json"]),
            false,
            "json: parse standard output.",
        ),
    ];
    static HTTP_GET: &[FieldSpec] = &[
        field(
            "url",
            FieldKind::Text,
            true,
            "An https address on a host in meta.http.",
        ),
        field("headers", FieldKind::Any, false, "Header names and values."),
        field(
            "timeout",
            FieldKind::Duration,
            false,
            "Such as 10s. Default 10s.",
        ),
        field(
            "format",
            FieldKind::Choice(&["json"]),
            false,
            "json: parse the body.",
        ),
        field(
            "credential",
            FieldKind::Text,
            false,
            "A host in meta.credentials.",
        ),
        field(
            "credential_scheme",
            FieldKind::Choice(&["bearer", "basic"]),
            false,
            "How the credential is sent. Default bearer.",
        ),
    ];
    static LOG: &[FieldSpec] = &[field("text", FieldKind::Text, true, "The line to keep.")];
    CtxSurface::from_functions(vec![
        CtxFn {
            path: "exec",
            level: Level::Read,
            outcomes: &["done", "failed", "timeout"],
            fields: EXEC,
            shape: ArgShape::Positional(&["argv"]),
        },
        CtxFn {
            path: "http.get",
            level: Level::Read,
            outcomes: &["done", "failed", "timeout"],
            fields: HTTP_GET,
            shape: ArgShape::Positional(&["url"]),
        },
        CtxFn {
            path: "log",
            level: Level::Read,
            outcomes: &["done"],
            fields: LOG,
            shape: ArgShape::Positional(&["text"]),
        },
    ])
}

const fn field(
    name: &'static str,
    kind: FieldKind,
    required: bool,
    help: &'static str,
) -> FieldSpec {
    FieldSpec {
        name,
        kind,
        required,
        help,
    }
}

/// Reads one extension file: its text, hash, `meta` and slots, with every problem found.
pub fn load_file(
    engine: &dyn ScriptEngine,
    path: &Path,
) -> Result<Extension, Vec<ScriptDiagnostic>> {
    let line_zero = |message: String| vec![ScriptDiagnostic { line: 0, message }];
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_FILE) {
        return Err(line_zero("file is larger than 256 KiB".to_owned()));
    }
    let bytes = std::fs::read(path).map_err(|e| line_zero(format!("cannot read the file: {e}")))?;
    let hash = crate::script::loader::sha256_hex(&bytes);
    let source =
        String::from_utf8(bytes).map_err(|_| line_zero("file is not valid UTF-8".to_owned()))?;
    let shape = engine.read_extension(&file_name, &source, PRELUDE)?;
    let meta = parse_meta(&shape.meta)
        .map_err(|problems| problems.into_iter().flat_map(line_zero).collect::<Vec<_>>())?;
    let mut problems = Vec::new();
    for slot in &shape.slots {
        if !SLOTS.contains(&slot.as_str()) {
            problems.push(format!(
                "slots.{slot} is not a slot; valid slots: {}",
                SLOTS.join(", ")
            ));
        }
    }
    if shape.slots.is_empty() {
        problems.push(format!(
            "M.slots claims no slot; valid slots: {}",
            SLOTS.join(", ")
        ));
    }
    if !problems.is_empty() {
        return Err(problems.into_iter().flat_map(line_zero).collect());
    }
    Ok(Extension {
        path: path.to_path_buf(),
        file_name,
        hash,
        source,
        meta,
        slots: shape.slots,
    })
}

/// Checks `M.meta` and reads it. Every problem is listed, each naming the field.
pub fn parse_meta(value: &Value) -> Result<Meta, Vec<String>> {
    const KEYS: &[&str] = &["name", "description", "api", "exec", "http", "credentials"];
    let mut problems = Vec::new();
    let Some(map) = value.as_object() else {
        return Err(vec!["M.meta must be a table".to_owned()]);
    };
    for key in map.keys() {
        if !KEYS.contains(&key.as_str()) {
            problems.push(format!(
                "meta.{key} is not a field; valid fields: {}",
                KEYS.join(", ")
            ));
        }
    }
    let name = match map.get("name").and_then(Value::as_str) {
        Some(name)
            if !name.is_empty()
                && name.len() <= 64
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
        {
            name.to_owned()
        }
        Some(_) => {
            problems.push("meta.name may hold only letters, digits and -".to_owned());
            String::new()
        }
        None => {
            problems.push("meta.name is missing".to_owned());
            String::new()
        }
    };
    let description = match map.get("description") {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(_) => {
            problems.push("meta.description must be a string".to_owned());
            String::new()
        }
    };
    let api = match map.get("api").and_then(Value::as_u64) {
        Some(api) if api == gm_schema::ext::API_VERSION => api,
        Some(api) => {
            problems.push(format!(
                "meta.api {api} is not known to this version of GitMaster, which speaks {}",
                gm_schema::ext::API_VERSION
            ));
            api
        }
        None => {
            problems.push(format!(
                "meta.api is missing; write api = {}",
                gm_schema::ext::API_VERSION
            ));
            0
        }
    };
    let mut list = |key: &str, check: fn(&str) -> bool, what: &str| -> Vec<String> {
        match map.get(key) {
            None => Vec::new(),
            Some(Value::Object(m)) if m.is_empty() => Vec::new(),
            Some(Value::Array(items)) => {
                let mut out = Vec::new();
                for item in items {
                    match item.as_str() {
                        Some(text) if check(text) => out.push(text.to_owned()),
                        _ => problems.push(format!("meta.{key} must be a list of {what}")),
                    }
                }
                out
            }
            Some(_) => {
                problems.push(format!("meta.{key} must be a list of {what}"));
                Vec::new()
            }
        }
    };
    let exec = list("exec", is_program, "program names");
    let http = list("http", is_host, "host names, such as code.example.com");
    let credentials = list("credentials", is_host, "host names");
    for host in &credentials {
        if !http.contains(host) {
            problems.push(format!(
                "meta.credentials names {host}, which is not in meta.http"
            ));
        }
    }
    problems.dedup();
    if problems.is_empty() {
        Ok(Meta {
            name,
            description,
            api,
            exec,
            http,
            credentials,
        })
    } else {
        Err(problems)
    }
}

/// A program name or path, not an option.
fn is_program(text: &str) -> bool {
    !text.is_empty() && !text.starts_with('-') && !text.contains(char::is_whitespace)
}

/// An exact, lowercase DNS name: no scheme, port, path or wildcard.
fn is_host(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 253
        && text.contains('.')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !text.starts_with(['.', '-'])
        && !text.ends_with(['.', '-'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_meta_reads() {
        let meta = parse_meta(&json!({
            "name": "pull-request", "api": 1, "description": "d",
            "exec": ["gh"], "http": ["code.example.com"], "credentials": ["code.example.com"],
        }))
        .unwrap();
        assert_eq!(meta.name, "pull-request");
        assert_eq!(meta.exec, vec!["gh"]);
        assert_eq!(meta.credentials, vec!["code.example.com"]);
    }

    #[test]
    fn every_problem_is_named() {
        let problems = parse_meta(&json!({
            "name": "bad name", "api": 2, "http": ["*.example.com", "https://x.example"],
            "credentials": ["other.example"], "colour": "red",
        }))
        .unwrap_err();
        assert!(problems.contains(&"meta.name may hold only letters, digits and -".to_owned()));
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("meta.api 2 is not known"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("meta.http must be a list of host names"))
        );
        assert!(problems.contains(
            &"meta.credentials names other.example, which is not in meta.http".to_owned()
        ));
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("meta.colour is not a field"))
        );
    }

    #[test]
    fn hosts_are_exact_names() {
        assert!(is_host("api.code.example.com"));
        for bad in [
            "",
            "localhost",
            "*.example.com",
            "Example.com",
            "x.example:443",
            "x.example/",
            ".x.example",
        ] {
            assert!(!is_host(bad), "{bad}");
        }
    }
}
