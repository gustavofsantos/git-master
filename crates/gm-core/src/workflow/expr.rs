//! The expression language of workflows (design/workflow-scripts.md, D62, D78): `${path}`
//! references inside templates, and conditions with comparisons,
//! `and`/`or`/`not`, parentheses and `len()`. Pure; no I/O, no variables.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value;

/// What expressions can read while a run is evaluated.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    pub inputs: &'a Value,
    pub trigger: &'a Value,
    pub repo: &'a Value,
    /// Output of each state that has run, by state id (`states.<id>.output`).
    pub states: &'a BTreeMap<String, Value>,
    /// The current `for_each` item: its name and value.
    pub item: Option<(&'a str, &'a Value)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprError(pub String);

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ExprError {}

/// Every `${...}` path in a template or a condition, split on dots, in
/// order of appearance: `${states.plan.output.plan_id}` gives
/// `["states", "plan", "output", "plan_id"]`.
pub fn references(source: &str) -> Result<Vec<Vec<String>>, ExprError> {
    Ok(split_template(source)?
        .into_iter()
        .filter_map(|segment| match segment {
            Segment::Ref(path) => Some(path),
            Segment::Text(_) => None,
        })
        .collect())
}

/// Renders one template string. A string that is exactly one `${path}`
/// yields the referenced value with its type; any other string yields a
/// string with each reference written in.
pub fn render(template: &str, scope: &Scope<'_>) -> Result<Value, ExprError> {
    let segments = split_template(template)?;
    if let [Segment::Ref(path)] = segments.as_slice() {
        return Ok(resolve(path, scope)?.clone());
    }
    let mut out = String::new();
    for segment in &segments {
        match segment {
            Segment::Text(text) => out.push_str(text),
            Segment::Ref(path) => match resolve(path, scope)? {
                Value::String(text) => out.push_str(text),
                Value::Null => return Err(ExprError(format!("`{}` is null", path.join(".")))),
                other => out.push_str(&other.to_string()),
            },
        }
    }
    Ok(Value::String(out))
}

/// Renders every string inside a `with` value, keeping its shape.
pub fn render_value(with: &Value, scope: &Scope<'_>) -> Result<Value, ExprError> {
    match with {
        Value::String(template) => render(template, scope),
        Value::Array(items) => items
            .iter()
            .map(|item| render_value(item, scope))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(key, item)| Ok((key.clone(), render_value(item, scope)?)))
            .collect::<Result<serde_json::Map<String, Value>, ExprError>>()
            .map(Value::Object),
        other => Ok(other.clone()),
    }
}

/// Checks the syntax of a condition without evaluating it.
pub fn check_condition(source: &str) -> Result<(), ExprError> {
    parse_condition(source).map(|_| ())
}

/// Evaluates a condition to a boolean.
pub fn eval_condition(source: &str, scope: &Scope<'_>) -> Result<bool, ExprError> {
    let node = parse_condition(source)?;
    match eval(&node, scope)?.as_ref() {
        Value::Bool(value) => Ok(*value),
        _ => Err(ExprError("condition is not a boolean".to_owned())),
    }
}

enum Segment<'t> {
    Text(&'t str),
    Ref(Vec<String>),
}

/// Splits a template into literal text and parsed `${...}` references.
fn split_template(template: &str) -> Result<Vec<Segment<'_>>, ExprError> {
    let mut segments = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        if start > 0 {
            segments.push(Segment::Text(&rest[..start]));
        }
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| ExprError("unclosed `${`".to_owned()))?;
        segments.push(Segment::Ref(parse_path(&after[..end])?));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Text(rest));
    }
    Ok(segments)
}

fn parse_path(inner: &str) -> Result<Vec<String>, ExprError> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Err(ExprError("empty reference".to_owned()));
    }
    let mut path = Vec::new();
    for (index, segment) in trimmed.split('.').enumerate() {
        let valid = is_ident(segment) || (index > 0 && is_digits(segment));
        if !valid {
            return Err(ExprError(format!("invalid reference path `{trimmed}`")));
        }
        path.push(segment.to_owned());
    }
    Ok(path)
}

/// A name a `${...}` path can hold: letters, digits and `_`, not starting
/// with a digit. State ids follow it, so every state can be referenced.
pub(crate) fn is_ident(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Looks a reference up in the scope. The fixed roots win over a
/// `for_each` item of the same name.
fn resolve<'a>(path: &[String], scope: &Scope<'a>) -> Result<&'a Value, ExprError> {
    let full = path.join(".");
    let (root, rest) = path
        .split_first()
        .ok_or_else(|| ExprError("empty reference".to_owned()))?;
    let base = match root.as_str() {
        "inputs" => scope.inputs,
        "trigger" => scope.trigger,
        "repo" => scope.repo,
        "states" => return resolve_state(rest, &full, scope),
        _ => match scope.item {
            Some((name, value)) if name == root => value,
            _ => return Err(ExprError(format!("unknown reference `{root}`"))),
        },
    };
    navigate(base, rest, &full)
}

fn resolve_state<'a>(
    rest: &[String],
    full: &str,
    scope: &Scope<'a>,
) -> Result<&'a Value, ExprError> {
    let (id, after) = rest
        .split_first()
        .ok_or_else(|| ExprError("reference `states` needs a state id".to_owned()))?;
    let output = scope
        .states
        .get(id)
        .ok_or_else(|| ExprError(format!("state `{id}` has not run")))?;
    match after.split_first() {
        Some((first, tail)) if first == "output" => navigate(output, tail, full),
        _ => Err(ExprError(format!(
            "reference `{full}` must read the `output` of its state"
        ))),
    }
}

fn navigate<'a>(base: &'a Value, path: &[String], full: &str) -> Result<&'a Value, ExprError> {
    let mut current = base;
    for segment in path {
        current = match current {
            Value::Object(map) => map.get(segment),
            Value::Array(items) => segment
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index)),
            _ => None,
        }
        .ok_or_else(|| ExprError(format!("reference `{full}` is missing")))?;
    }
    Ok(current)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    LParen,
    RParen,
    Cmp(CmpOp),
    And,
    Or,
    Not,
    Len,
    Ref(Vec<String>),
    Literal(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

const OPERATORS: &[(&str, CmpOp)] = &[
    ("==", CmpOp::Eq),
    ("!=", CmpOp::Ne),
    ("<=", CmpOp::Le),
    (">=", CmpOp::Ge),
    ("<", CmpOp::Lt),
    (">", CmpOp::Gt),
];

fn tokenize(source: &str) -> Result<Vec<Token>, ExprError> {
    let mut tokens = Vec::new();
    let mut rest = source;
    loop {
        rest = rest.trim_start();
        let Some(first) = rest.chars().next() else {
            break;
        };
        if let Some(after) = rest.strip_prefix("${") {
            let end = after
                .find('}')
                .ok_or_else(|| ExprError("unclosed `${`".to_owned()))?;
            tokens.push(Token::Ref(parse_path(&after[..end])?));
            rest = &after[end + 1..];
            continue;
        }
        let (token, len) = match first {
            '(' => (Token::LParen, 1),
            ')' => (Token::RParen, 1),
            '\'' | '"' => {
                let (text, len) = read_string(rest)?;
                (Token::Literal(Value::String(text)), len)
            }
            '-' | '0'..='9' => read_number(rest)?,
            'a'..='z' | 'A'..='Z' | '_' => read_word(rest)?,
            _ => read_operator(rest, first)?,
        };
        tokens.push(token);
        rest = &rest[len..];
    }
    Ok(tokens)
}

/// Reads a quoted string at the start of `rest`; returns its text and the
/// number of bytes consumed, quotes included.
fn read_string(rest: &str) -> Result<(String, usize), ExprError> {
    let quote = rest.as_bytes()[0] as char;
    let mut out = String::new();
    let mut escaped = false;
    for (index, c) in rest[1..].char_indices() {
        if escaped {
            match c {
                '\'' | '"' | '\\' => out.push(c),
                other => return Err(ExprError(format!("unknown escape `\\{other}`"))),
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return Ok((out, index + 2));
        } else {
            out.push(c);
        }
    }
    Err(ExprError("unclosed string".to_owned()))
}

fn read_number(rest: &str) -> Result<(Token, usize), ExprError> {
    let negative = rest.starts_with('-');
    let body = &rest[usize::from(negative)..];
    let run = body
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(body.len());
    let text = &rest[..usize::from(negative) + run];
    let digits = &body[..run];
    if digits.is_empty()
        || digits.starts_with('.')
        || digits.ends_with('.')
        || digits.matches('.').count() > 1
    {
        return Err(ExprError(format!("invalid number `{text}`")));
    }
    let value = if digits.contains('.') {
        text.parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
    } else {
        text.parse::<i64>().ok().map(Value::from)
    };
    let value = value.ok_or_else(|| ExprError(format!("number `{text}` is out of range")))?;
    Ok((Token::Literal(value), text.len()))
}

fn read_word(rest: &str) -> Result<(Token, usize), ExprError> {
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let token = match &rest[..len] {
        "and" => Token::And,
        "or" => Token::Or,
        "not" => Token::Not,
        "len" => Token::Len,
        "true" => Token::Literal(Value::Bool(true)),
        "false" => Token::Literal(Value::Bool(false)),
        "null" => Token::Literal(Value::Null),
        word => {
            return Err(ExprError(format!(
                "unknown word `{word}` (a value is written as `${{path}}`, a number or a string)"
            )));
        }
    };
    Ok((token, len))
}

fn read_operator(rest: &str, first: char) -> Result<(Token, usize), ExprError> {
    for (symbol, op) in OPERATORS {
        if rest.starts_with(symbol) {
            return Ok((Token::Cmp(*op), symbol.len()));
        }
    }
    Err(ExprError(format!("unexpected `{first}`")))
}

fn parse_condition(source: &str) -> Result<Node, ExprError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
    };
    let node = parser.parse_or()?;
    match parser.peek() {
        None => Ok(node),
        Some(token) => Err(ExprError(format!(
            "unexpected {} after the condition",
            describe(Some(token))
        ))),
    }
}

#[derive(Debug)]
enum Node {
    Literal(Value),
    Ref(Vec<String>),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Cmp(CmpOp, Box<Node>, Box<Node>),
    Len(Box<Node>),
}

struct Parser<'t> {
    tokens: &'t [Token],
    pos: usize,
}

impl<'t> Parser<'t> {
    fn peek(&self) -> Option<&'t Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<&'t Token> {
        let token = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(token)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ExprError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(ExprError(format!(
                "expected {}, found {}",
                describe(Some(expected)),
                describe(self.peek())
            )))
        }
    }

    fn parse_or(&mut self) -> Result<Node, ExprError> {
        let mut left = self.parse_and()?;
        while self.eat(&Token::Or) {
            let right = self.parse_and()?;
            left = Node::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Node, ExprError> {
        let mut left = self.parse_not()?;
        while self.eat(&Token::And) {
            let right = self.parse_not()?;
            left = Node::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Node, ExprError> {
        if self.eat(&Token::Not) {
            return Ok(Node::Not(Box::new(self.parse_not()?)));
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> Result<Node, ExprError> {
        let left = self.parse_primary()?;
        if let Some(Token::Cmp(op)) = self.peek() {
            let op = *op;
            self.pos += 1;
            let right = self.parse_primary()?;
            return Ok(Node::Cmp(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Node, ExprError> {
        let Some(token) = self.bump() else {
            return Err(ExprError(
                "expected a value, found the end of the condition".to_owned(),
            ));
        };
        match token {
            Token::LParen => {
                let inner = self.parse_or()?;
                self.expect(&Token::RParen)?;
                Ok(inner)
            }
            Token::Len => {
                self.expect(&Token::LParen)?;
                let inner = self.parse_or()?;
                self.expect(&Token::RParen)?;
                Ok(Node::Len(Box::new(inner)))
            }
            Token::Ref(path) => Ok(Node::Ref(path.clone())),
            Token::Literal(value) => Ok(Node::Literal(value.clone())),
            other => Err(ExprError(format!(
                "expected a value, found {}",
                describe(Some(other))
            ))),
        }
    }
}

fn describe(token: Option<&Token>) -> String {
    match token {
        None => "the end of the condition".to_owned(),
        Some(Token::LParen) => "`(`".to_owned(),
        Some(Token::RParen) => "`)`".to_owned(),
        Some(Token::Cmp(op)) => format!("`{}`", op.symbol()),
        Some(Token::And) => "`and`".to_owned(),
        Some(Token::Or) => "`or`".to_owned(),
        Some(Token::Not) => "`not`".to_owned(),
        Some(Token::Len) => "`len`".to_owned(),
        Some(Token::Ref(path)) => format!("`${{{}}}`", path.join(".")),
        Some(Token::Literal(value)) => format!("`{value}`"),
    }
}

/// Evaluates a node. Borrows from the scope where it can, so a reference
/// to a large output is not copied just to be compared.
fn eval<'a>(node: &Node, scope: &Scope<'a>) -> Result<Cow<'a, Value>, ExprError> {
    let value = match node {
        Node::Literal(value) => value.clone(),
        Node::Ref(path) => return Ok(Cow::Borrowed(resolve(path, scope)?)),
        Node::Not(inner) => Value::Bool(!eval_truth(inner, scope, "not")?),
        Node::And(left, right) => {
            Value::Bool(eval_truth(left, scope, "and")? && eval_truth(right, scope, "and")?)
        }
        Node::Or(left, right) => {
            Value::Bool(eval_truth(left, scope, "or")? || eval_truth(right, scope, "or")?)
        }
        Node::Cmp(op, left, right) => {
            let left = eval(left, scope)?;
            let right = eval(right, scope)?;
            Value::Bool(compare(*op, &left, &right)?)
        }
        Node::Len(inner) => {
            let inner = eval(inner, scope)?;
            Value::from(length(&inner)?)
        }
    };
    Ok(Cow::Owned(value))
}

fn eval_truth(node: &Node, scope: &Scope<'_>, op: &str) -> Result<bool, ExprError> {
    let value = eval(node, scope)?;
    truth(&value, op)
}

fn truth(value: &Value, op: &str) -> Result<bool, ExprError> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(ExprError(format!(
            "`{op}` needs a boolean, found {}",
            type_name(other)
        ))),
    }
}

fn length(value: &Value) -> Result<usize, ExprError> {
    match value {
        Value::String(text) => Ok(text.chars().count()),
        Value::Array(items) => Ok(items.len()),
        Value::Object(map) => Ok(map.len()),
        other => Err(ExprError(format!(
            "`len` needs a string, array or object, found {}",
            type_name(other)
        ))),
    }
}

fn compare(op: CmpOp, left: &Value, right: &Value) -> Result<bool, ExprError> {
    match op {
        CmpOp::Eq => Ok(same(left, right)),
        CmpOp::Ne => Ok(!same(left, right)),
        CmpOp::Lt => Ok(order(left, right)?.is_lt()),
        CmpOp::Le => Ok(order(left, right)?.is_le()),
        CmpOp::Gt => Ok(order(left, right)?.is_gt()),
        CmpOp::Ge => Ok(order(left, right)?.is_ge()),
    }
}

/// JSON equality, except that numbers compare by value (`1 == 1.0`).
fn same(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same(value, other)))
        }
        _ => left == right,
    }
}

fn order(left: &Value, right: &Value) -> Result<Ordering, ExprError> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .zip(b.as_f64())
            .and_then(|(a, b)| a.partial_cmp(&b))
            .ok_or_else(|| ExprError("cannot order these numbers".to_owned())),
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        _ => Err(ExprError(format!(
            "cannot order {} and {}",
            type_name(left),
            type_name(right)
        ))),
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    struct Fixture {
        inputs: Value,
        trigger: Value,
        repo: Value,
        states: BTreeMap<String, Value>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                inputs: json!({"name": "gm", "count": 3, "list": [1, 2], "nothing": null}),
                trigger: json!({"root": "main", "flag": true}),
                repo: json!({"owner": "x"}),
                states: BTreeMap::from([
                    (
                        "plan".to_owned(),
                        json!({"branch_count": 0, "plan_id": "p1"}),
                    ),
                    ("ask".to_owned(), json!({"text": ""})),
                ]),
            }
        }

        fn scope(&self) -> Scope<'_> {
            Scope {
                inputs: &self.inputs,
                trigger: &self.trigger,
                repo: &self.repo,
                states: &self.states,
                item: None,
            }
        }
    }

    fn path(text: &str) -> Vec<String> {
        text.split('.').map(str::to_owned).collect()
    }

    fn message<T: std::fmt::Debug>(result: Result<T, ExprError>) -> String {
        match result {
            Err(error) => error.0,
            Ok(value) => panic!("expected an error, got {value:?}"),
        }
    }

    #[test]
    fn references_of_template_and_condition() {
        assert_eq!(
            references("run ${inputs.name} for ${states.plan.output.plan_id}"),
            Ok(vec![
                path("inputs.name"),
                path("states.plan.output.plan_id")
            ])
        );
        assert_eq!(
            references("${states.plan.output.branch_count} == 0 and not ${ trigger.flag }"),
            Ok(vec![
                path("states.plan.output.branch_count"),
                path("trigger.flag")
            ])
        );
        assert_eq!(references("no references"), Ok(Vec::new()));
    }

    #[test]
    fn references_reject_bad_paths() {
        for source in ["${a..b}", "${ }", "${1a}", "${0}", "${a.}", "${a"] {
            assert!(references(source).is_err(), "{source}");
        }
    }

    #[test]
    fn render_keeps_type_for_a_single_reference() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(render("${inputs.count}", &scope), Ok(json!(3)));
        assert_eq!(render("${ trigger }", &scope), Ok(fixture.trigger.clone()));
        assert_eq!(render("${repo}", &scope), Ok(json!({"owner": "x"})));
        assert_eq!(render("${inputs.nothing}", &scope), Ok(Value::Null));
    }

    #[test]
    fn render_concatenates_references_as_text() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(
            render(
                "run ${inputs.name} #${inputs.count}, ${trigger.flag}: ${inputs.list}",
                &scope
            ),
            Ok(json!("run gm #3, true: [1,2]"))
        );
        assert_eq!(
            render("${inputs.count}${inputs.count}", &scope),
            Ok(json!("33"))
        );
        assert_eq!(
            render("no $ sign here", &scope),
            Ok(json!("no $ sign here"))
        );
        assert_eq!(render("", &scope), Ok(json!("")));
    }

    #[test]
    fn render_rejects_null_inside_text() {
        let fixture = Fixture::new();
        assert_eq!(
            message(render("x ${inputs.nothing}", &fixture.scope())),
            "`inputs.nothing` is null"
        );
    }

    #[test]
    fn render_reports_a_state_that_has_not_run() {
        let fixture = Fixture::new();
        assert_eq!(
            message(render("${states.review.output.text}", &fixture.scope())),
            "state `review` has not run"
        );
    }

    #[test]
    fn states_must_read_output() {
        let fixture = Fixture::new();
        assert_eq!(
            message(render("${states.plan.plan_id}", &fixture.scope())),
            "reference `states.plan.plan_id` must read the `output` of its state"
        );
        assert_eq!(
            render("${states.plan.output.plan_id}", &fixture.scope()),
            Ok(json!("p1"))
        );
    }

    #[test]
    fn item_resolves_into_the_current_for_each_value() {
        let fixture = Fixture::new();
        let item = json!({"name": "a", "tags": ["x", "y"]});
        let scope = Scope {
            item: Some(("item", &item)),
            ..fixture.scope()
        };
        assert_eq!(
            render("${item.name} ${item.tags.1}", &scope),
            Ok(json!("a y"))
        );
        assert_eq!(render("${item}", &scope), Ok(item.clone()));
        assert_eq!(
            message(render("${other.name}", &scope)),
            "unknown reference `other`"
        );
    }

    #[test]
    fn array_index_and_missing_paths() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(render("${inputs.list.0}", &scope), Ok(json!(1)));
        assert_eq!(
            message(render("${inputs.list.5}", &scope)),
            "reference `inputs.list.5` is missing"
        );
        assert_eq!(
            message(render("${inputs.missing.deep}", &scope)),
            "reference `inputs.missing.deep` is missing"
        );
        assert_eq!(
            message(render("${foo.bar}", &scope)),
            "unknown reference `foo`"
        );
    }

    #[test]
    fn render_value_walks_objects_and_arrays() {
        let fixture = Fixture::new();
        let with = json!({
            "a": "${inputs.count}",
            "b": ["x ${inputs.name}", 2, true],
            "c": null
        });
        assert_eq!(
            render_value(&with, &fixture.scope()),
            Ok(json!({"a": 3, "b": ["x gm", 2, true], "c": null}))
        );
    }

    #[test]
    fn condition_reads_a_state_output() {
        let mut fixture = Fixture::new();
        assert_eq!(
            eval_condition("${states.plan.output.branch_count} == 0", &fixture.scope()),
            Ok(true)
        );
        fixture
            .states
            .insert("plan".to_owned(), json!({"branch_count": 2}));
        assert_eq!(
            eval_condition("${states.plan.output.branch_count} == 0", &fixture.scope()),
            Ok(false)
        );
    }

    #[test]
    fn condition_len_of_a_state_text() {
        let fixture = Fixture::new();
        assert_eq!(
            eval_condition("len(${states.ask.output.text}) == 0", &fixture.scope()),
            Ok(true)
        );
    }

    #[test]
    fn condition_or_of_two_comparisons() {
        let mut fixture = Fixture::new();
        let source = "${trigger.root} == 'main' or ${trigger.root} == 'release'";
        assert_eq!(eval_condition(source, &fixture.scope()), Ok(true));
        fixture.trigger = json!({"root": "release"});
        assert_eq!(eval_condition(source, &fixture.scope()), Ok(true));
        fixture.trigger = json!({"root": "dev"});
        assert_eq!(eval_condition(source, &fixture.scope()), Ok(false));
    }

    #[test]
    fn precedence_and_grouping() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(eval_condition("not true and false", &scope), Ok(false));
        assert_eq!(eval_condition("true or false and false", &scope), Ok(true));
        assert_eq!(eval_condition("false and false or true", &scope), Ok(true));
        assert_eq!(
            eval_condition("(true or false) and false", &scope),
            Ok(false)
        );
        assert_eq!(eval_condition("true and (false or true)", &scope), Ok(true));
        assert_eq!(eval_condition("not (1 == 2)", &scope), Ok(true));
        assert_eq!(eval_condition("not not true", &scope), Ok(true));
    }

    #[test]
    fn comparisons_of_each_type() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(eval_condition("1 == 1.0", &scope), Ok(true));
        assert_eq!(eval_condition("-2 < 1.5", &scope), Ok(true));
        assert_eq!(eval_condition("2 != 2.5", &scope), Ok(true));
        assert_eq!(eval_condition("'a' < 'b'", &scope), Ok(true));
        assert_eq!(eval_condition("'abc' >= 'abd'", &scope), Ok(false));
        assert_eq!(eval_condition("null == null", &scope), Ok(true));
        assert_eq!(eval_condition("1 == 'a'", &scope), Ok(false));
        assert_eq!(
            eval_condition("${inputs.list} == ${inputs.list}", &scope),
            Ok(true)
        );
        assert_eq!(eval_condition("len('héllo') == 5", &scope), Ok(true));
        assert_eq!(eval_condition("len(${inputs.list}) == 2", &scope), Ok(true));
        assert_eq!(eval_condition("len(${repo}) == 1", &scope), Ok(true));
    }

    #[test]
    fn short_circuit_skips_the_right_side() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(
            eval_condition("false and ${missing.x} == 1", &scope),
            Ok(false)
        );
        assert_eq!(
            eval_condition("true or ${missing.x} == 1", &scope),
            Ok(true)
        );
        assert_eq!(
            message(eval_condition("${missing.x} == 1", &scope)),
            "unknown reference `missing`"
        );
        assert_eq!(
            message(eval_condition("true and ${inputs.missing} == 1", &scope)),
            "reference `inputs.missing` is missing"
        );
    }

    #[test]
    fn type_errors_are_reported() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(
            message(eval_condition("1 < 'a'", &scope)),
            "cannot order number and string"
        );
        assert_eq!(
            message(eval_condition("true < false", &scope)),
            "cannot order boolean and boolean"
        );
        assert_eq!(
            message(eval_condition("'a' == 'a' and 1", &scope)),
            "`and` needs a boolean, found number"
        );
        assert_eq!(
            message(eval_condition("not 1", &scope)),
            "`not` needs a boolean, found number"
        );
        assert_eq!(
            message(eval_condition("len(3) == 1", &scope)),
            "`len` needs a string, array or object, found number"
        );
        assert_eq!(
            message(eval_condition("1", &scope)),
            "condition is not a boolean"
        );
    }

    #[test]
    fn string_escapes() {
        let fixture = Fixture::new();
        let scope = fixture.scope();
        assert_eq!(eval_condition(r#"'it\'s' == "it's""#, &scope), Ok(true));
        assert_eq!(
            eval_condition(r#""back\\slash" == 'back\\slash'"#, &scope),
            Ok(true)
        );
    }

    #[test]
    fn syntax_errors_are_reported() {
        for source in [
            "${ a",
            "1 ==",
            "'abc",
            "1 = 1",
            "a == 1",
            "(1 == 1",
            "1 == 1)",
            "${} == 1",
            "len 1 == 1",
            "1 == 2 == true",
            "",
            "true and",
            "1.5.5 == 1",
            "-",
            "$ == 1",
            "'bad \\q' == 1",
        ] {
            assert!(check_condition(source).is_err(), "{source}");
            assert!(
                eval_condition(source, &Fixture::new().scope()).is_err(),
                "{source}"
            );
        }
        assert_eq!(message(check_condition("${ a")), "unclosed `${`");
        assert_eq!(
            message(check_condition("1 ==")),
            "expected a value, found the end of the condition"
        );
        assert_eq!(message(check_condition("'abc")), "unclosed string");
    }

    #[test]
    fn valid_conditions_pass_the_syntax_check() {
        assert_eq!(
            check_condition("${a.b} == 'x' and not (len(${c}) < 3)"),
            Ok(())
        );
    }

    proptest! {
        #[test]
        fn text_without_a_dollar_renders_to_itself(text in "[^$]*") {
            let fixture = Fixture::new();
            prop_assert_eq!(render(&text, &fixture.scope()), Ok(Value::String(text.clone())));
        }

        #[test]
        fn integer_less_than_matches_rust(n in any::<i32>(), m in any::<i32>()) {
            let fixture = Fixture::new();
            prop_assert_eq!(
                eval_condition(&format!("{n} < {m}"), &fixture.scope()),
                Ok(n < m)
            );
        }
    }
}
