//! `ctx.http.get` of an extension (D254 (1)): HTTPS `GET` to a host the extension lists, redirects
//! only to listed hosts, a capped body, and a Git credential the host adds and the script never
//! sees. The network and the credential helper sit behind ports so the rules are tested offline.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Map, Value, json};

/// The largest body kept.
pub const BODY_LIMIT: usize = 1024 * 1024;
/// The most redirects followed.
const MAX_REDIRECTS: usize = 5;
/// Headers a script may not set: the host owns them.
const RESERVED_HEADERS: &[&str] = &["authorization", "host", "cookie", "proxy-authorization"];

/// One request as the network port sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
}

/// One response, before redirects are followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Lowercase names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub truncated: bool,
}

/// Sends one request without following redirects.
pub trait Network: Send + Sync {
    fn get(&self, request: &Request) -> Result<Response, Failure>;
}

/// Why a request did not get a response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Timeout,
    Other(String),
}

/// Looks up a credential for a host, never prompting.
pub trait Credentials: Send + Sync {
    /// `(username, password)`, or `None` when there is none.
    fn lookup(&self, host: &str) -> Option<(String, String)>;
}

/// The checked parts of a URL.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Url {
    host: String,
}

/// Accepts only `https://<host>[:port]/...`; returns the host, lowercase.
fn parse_url(url: &str) -> Result<Url, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("only https addresses are allowed: {url}"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("an address may not carry a user or password".to_owned());
    }
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                host
            } else {
                authority
            }
        })
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err(format!("the address has no host: {url}"));
    }
    Ok(Url { host })
}

/// What a script asked, read from the call's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Get {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
    pub json: bool,
    pub credential: Option<String>,
    pub basic: bool,
}

/// Reads the arguments of `ctx.http.get`.
pub fn read_args(args: &Value, default_timeout: Duration) -> Result<Get, String> {
    const KEYS: &[&str] = &[
        "url",
        "headers",
        "timeout",
        "format",
        "credential",
        "credential_scheme",
    ];
    let map = args
        .as_object()
        .ok_or("ctx.http.get takes a url and a table")?;
    if let Some(key) = map.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!(
            "ctx.http.get has no option `{key}`; options: {}",
            KEYS[1..].join(", ")
        ));
    }
    let url = map
        .get("url")
        .and_then(Value::as_str)
        .ok_or("ctx.http.get needs a url")?
        .to_owned();
    let mut headers = Vec::new();
    match map.get("headers") {
        None => {}
        Some(Value::Object(h)) => {
            for (name, value) in h {
                let value = value
                    .as_str()
                    .ok_or_else(|| format!("header `{name}` must be a string"))?;
                if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(format!(
                        "header `{name}` is set by GitMaster, not by a script"
                    ));
                }
                if name.is_empty()
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    || value.contains(['\r', '\n'])
                {
                    return Err(format!("header `{name}` is not a valid header"));
                }
                headers.push((name.clone(), value.to_owned()));
            }
        }
        Some(_) => return Err("`headers` must be a table of names and values".to_owned()),
    }
    let timeout = match map.get("timeout") {
        None => default_timeout,
        Some(Value::String(text)) => crate::script::exec::parse_timeout(text)?,
        Some(_) => return Err("`timeout` must be a string such as \"10s\"".to_owned()),
    };
    let json = match map.get("format") {
        None => false,
        Some(value) if value.as_str() == Some("json") => true,
        Some(_) => return Err("`format` must be \"json\"".to_owned()),
    };
    let credential = match map.get("credential") {
        None => None,
        Some(Value::String(host)) => Some(host.clone()),
        Some(_) => return Err("`credential` must be a host name".to_owned()),
    };
    let basic = match map.get("credential_scheme").and_then(Value::as_str) {
        None | Some("bearer") => false,
        Some("basic") => true,
        Some(other) => {
            return Err(format!(
                "`credential_scheme` must be bearer or basic, not {other}"
            ));
        }
    };
    Ok(Get {
        url,
        headers,
        timeout,
        json,
        credential,
        basic,
    })
}

/// How a `get` ended, as the host passes it to the script.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done(Value),
    Timeout,
    /// A refusal or a network failure. The message never holds a credential.
    Failed(String),
}

/// Runs a `get` under the extension's lists. `allowed` is `meta.http`, `credential_hosts` is
/// `meta.credentials`.
pub fn get(
    get: &Get,
    allowed: &[String],
    credential_hosts: &[String],
    network: &dyn Network,
    credentials: &dyn Credentials,
) -> Outcome {
    let first = match parse_url(&get.url) {
        Ok(url) => url,
        Err(message) => return Outcome::Failed(message),
    };
    if !allowed.contains(&first.host) {
        return Outcome::Failed(format!(
            "{} is not in meta.http; add it there to reach it",
            first.host
        ));
    }
    let authorization = match &get.credential {
        None => None,
        Some(host) if !credential_hosts.contains(host) => {
            return Outcome::Failed(format!("{host} is not in meta.credentials"));
        }
        Some(host) => credentials.lookup(host).map(|(user, password)| {
            if get.basic {
                format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
            } else {
                format!("Bearer {password}")
            }
        }),
    };
    let mut url = get.url.clone();
    let mut host = first.host;
    for _ in 0..=MAX_REDIRECTS {
        let mut headers = get.headers.clone();
        // A credential goes only to the host it was given for.
        if let (Some(value), Some(for_host)) = (&authorization, &get.credential)
            && *for_host == host
        {
            headers.push(("Authorization".to_owned(), value.clone()));
        }
        let request = Request {
            url: url.clone(),
            headers,
            timeout: get.timeout,
        };
        let response = match network.get(&request) {
            Ok(response) => response,
            Err(Failure::Timeout) => return Outcome::Timeout,
            Err(Failure::Other(message)) => {
                return Outcome::Failed(redact(&message, authorization.as_ref()));
            }
        };
        if matches!(response.status, 301 | 302 | 303 | 307 | 308)
            && let Some((_, location)) = response.headers.iter().find(|(n, _)| n == "location")
        {
            let next = resolve(&url, location);
            match parse_url(&next) {
                Ok(target) if allowed.contains(&target.host) => {
                    url = next;
                    host = target.host;
                    continue;
                }
                Ok(target) => {
                    return Outcome::Failed(format!(
                        "the server redirected to {}, which is not in meta.http",
                        target.host
                    ));
                }
                Err(message) => {
                    return Outcome::Failed(format!("the server redirected: {message}"));
                }
            }
        }
        return Outcome::Done(result(&response, get.json));
    }
    Outcome::Failed(format!("more than {MAX_REDIRECTS} redirects"))
}

fn result(response: &Response, json: bool) -> Value {
    let text = String::from_utf8_lossy(&response.body).into_owned();
    let mut headers = Map::new();
    for (name, value) in &response.headers {
        if name != "set-cookie" {
            headers.insert(name.clone(), Value::String(value.clone()));
        }
    }
    let mut out = json!({
        "status": response.status,
        "text": text,
        "headers": headers,
        "truncated": response.truncated,
    });
    if json && !response.truncated {
        out["json"] = serde_json::from_slice::<Value>(&response.body).unwrap_or(Value::Null);
    }
    out
}

/// An absolute `Location`, or one relative to the origin of `base`.
fn resolve(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
    let after_scheme = base.find("://").map_or(0, |i| i + 3);
    let origin_end = base[after_scheme..]
        .find('/')
        .map_or(base.len(), |i| after_scheme + i);
    if location.starts_with('/') {
        format!("{}{location}", &base[..origin_end])
    } else {
        format!("{}/{location}", &base[..origin_end])
    }
}

fn redact(message: &str, authorization: Option<&String>) -> String {
    match authorization {
        Some(secret) if !secret.is_empty() => message.replace(secret.as_str(), "[credential]"),
        _ => message.to_owned(),
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The network through `ureq`, compiled into the binary.
pub struct UreqNetwork;

impl Network for UreqNetwork {
    fn get(&self, request: &Request) -> Result<Response, Failure> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(request.timeout))
            .http_status_as_error(false)
            .max_redirects(0)
            .https_only(true)
            .build()
            .into();
        let mut call = agent.get(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name.as_str(), value.as_str());
        }
        let mut response = call.call().map_err(|error| match error {
            ureq::Error::Timeout(_) => Failure::Timeout,
            other => Failure::Other(other.to_string()),
        })?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.as_str().to_ascii_lowercase(), v.to_owned()))
            })
            .collect();
        let mut body = Vec::new();
        let limit = u64::try_from(BODY_LIMIT).unwrap_or(u64::MAX);
        response
            .body_mut()
            .as_reader()
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| Failure::Other(e.to_string()))?;
        let truncated = body.len() > BODY_LIMIT;
        body.truncate(BODY_LIMIT);
        Ok(Response {
            status: response.status().as_u16(),
            headers,
            body,
            truncated,
        })
    }
}

/// Git's credential helper, as `git credential fill` asks it, never prompting.
pub struct GitCredentials;

impl Credentials for GitCredentials {
    fn lookup(&self, host: &str) -> Option<(String, String)> {
        let mut child = Command::new("git")
            .args(["-c", "credential.interactive=false", "credential", "fill"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = write!(stdin, "protocol=https\nhost={host}\n\n");
        }
        let output = child.wait_with_output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let field = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
                .map(str::to_owned)
        };
        let password = field("password").filter(|p| !p.is_empty())?;
        Some((field("username").unwrap_or_default(), password))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Script {
        responses: Mutex<Vec<Result<Response, Failure>>>,
        seen: Mutex<Vec<Request>>,
    }

    impl Script {
        fn new(responses: Vec<Result<Response, Failure>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl Network for Script {
        fn get(&self, request: &Request) -> Result<Response, Failure> {
            self.seen.lock().unwrap().push(request.clone());
            self.responses.lock().unwrap().remove(0)
        }
    }

    struct Token;
    impl Credentials for Token {
        fn lookup(&self, host: &str) -> Option<(String, String)> {
            (host == "code.example.com").then(|| ("me".to_owned(), "s3cret".to_owned()))
        }
    }

    fn ok(status: u16, body: &str, headers: &[(&str, &str)]) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            body: body.as_bytes().to_vec(),
            truncated: false,
        }
    }

    fn args(value: &Value) -> Get {
        read_args(value, Duration::from_secs(10)).unwrap()
    }

    fn hosts(list: &[&str]) -> Vec<String> {
        list.iter().map(|h| (*h).to_owned()).collect()
    }

    #[test]
    fn a_listed_host_answers_with_status_text_and_json() {
        let net = Script::new(vec![Ok(ok(
            200,
            r#"[{"number":4}]"#,
            &[("content-type", "application/json")],
        ))]);
        let out = get(
            &args(&json!({ "url": "https://code.example.com/api/pulls", "format": "json" })),
            &hosts(&["code.example.com"]),
            &[],
            &net,
            &Token,
        );
        let Outcome::Done(value) = out else {
            panic!("{out:?}")
        };
        assert_eq!(value["status"], 200);
        assert_eq!(value["json"][0]["number"], 4);
        assert!(net.seen.lock().unwrap()[0].headers.is_empty());
    }

    #[test]
    fn hosts_and_schemes_outside_the_list_are_refused_before_sending() {
        let net = Script::new(vec![]);
        for (url, needle) in [
            (
                "https://other.example/x",
                "other.example is not in meta.http",
            ),
            ("http://code.example.com/x", "only https"),
            ("https://user:pw@code.example.com/x", "user or password"),
        ] {
            let out = get(
                &args(&json!({ "url": url })),
                &hosts(&["code.example.com"]),
                &[],
                &net,
                &Token,
            );
            let Outcome::Failed(message) = out else {
                panic!("{url}")
            };
            assert!(message.contains(needle), "{message}");
        }
        assert!(net.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_credential_goes_only_to_its_host_and_never_to_the_script() {
        let net = Script::new(vec![
            Ok(ok(302, "", &[("location", "https://cdn.example.com/blob")])),
            Ok(ok(200, "body", &[])),
        ]);
        let out = get(
            &args(
                &json!({ "url": "https://code.example.com/a", "credential": "code.example.com" }),
            ),
            &hosts(&["code.example.com", "cdn.example.com"]),
            &hosts(&["code.example.com"]),
            &net,
            &Token,
        );
        let Outcome::Done(value) = out else {
            panic!("{out:?}")
        };
        assert!(!value.to_string().contains("s3cret"));
        let seen = net.seen.lock().unwrap();
        assert_eq!(
            seen[0].headers,
            vec![("Authorization".to_owned(), "Bearer s3cret".to_owned())]
        );
        assert_eq!(seen[1].url, "https://cdn.example.com/blob");
        assert!(
            seen[1].headers.is_empty(),
            "the credential must not follow a redirect to another host"
        );
    }

    #[test]
    fn a_redirect_off_the_list_is_refused() {
        let net = Script::new(vec![Ok(ok(
            301,
            "",
            &[("location", "https://evil.example/")],
        ))]);
        let out = get(
            &args(&json!({ "url": "https://code.example.com/a" })),
            &hosts(&["code.example.com"]),
            &[],
            &net,
            &Token,
        );
        assert_eq!(
            out,
            Outcome::Failed(
                "the server redirected to evil.example, which is not in meta.http".to_owned()
            )
        );
    }

    #[test]
    fn a_credential_needs_its_host_in_the_list() {
        let net = Script::new(vec![]);
        let out = get(
            &args(
                &json!({ "url": "https://code.example.com/a", "credential": "code.example.com" }),
            ),
            &hosts(&["code.example.com"]),
            &[],
            &net,
            &Token,
        );
        assert_eq!(
            out,
            Outcome::Failed("code.example.com is not in meta.credentials".to_owned())
        );
    }

    #[test]
    fn basic_is_encoded() {
        assert_eq!(base64(b"me:s3cret"), "bWU6czNjcmV0");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
    }

    #[test]
    fn reserved_and_broken_headers_are_refused() {
        for headers in [
            json!({ "Authorization": "x" }),
            json!({ "X-A": "a\r\nb" }),
            json!({ "bad name": "x" }),
        ] {
            assert!(
                read_args(
                    &json!({ "url": "https://a.example", "headers": headers }),
                    Duration::from_secs(1)
                )
                .is_err()
            );
        }
        assert!(
            read_args(
                &json!({ "url": "https://a.example", "method": "POST" }),
                Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn relative_locations_resolve_against_the_origin() {
        assert_eq!(
            resolve("https://a.example/x/y", "/z"),
            "https://a.example/z"
        );
        assert_eq!(resolve("https://a.example", "z"), "https://a.example/z");
        assert_eq!(
            resolve("https://a.example/x", "https://b.example/"),
            "https://b.example/"
        );
    }
}
