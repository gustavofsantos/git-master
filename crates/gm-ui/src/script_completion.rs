//! What the workflow script editor offers at the cursor (D151): the functions of `ctx` after
//! `ctx.`, and the keys of an argument table inside `ctx.<call>({ ... })`. Pure: the text and a
//! byte offset go in, items come out, so the rules are tested without a window. The editor's
//! completion provider only turns these items into the component's completion type.
//!
//! The names, signatures and documentation come from the sandbox's own surface
//! (`gm_core::script::surface`) and from the reference's text (`api_docs`), so the editor cannot
//! offer a call the engine does not have.

use gm_core::script::api_docs;
use gm_core::script::surface::{ArgShape, CtxFn, CtxSurface};

/// What an item is, for the editor's icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// A function of `ctx`: an action or a helper.
    Function,
    /// A group of functions, such as `branch` in `ctx.branch.create`.
    Namespace,
    /// A key of an argument table, such as `name` in `ctx.branch.create({ name = ... })`.
    Field,
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The text inserted, and matched against what was typed.
    pub label: String,
    pub kind: ItemKind,
    /// One line: the signature of a function, or the type of a key.
    pub detail: String,
    /// Markdown: what the reference says about it.
    pub documentation: String,
}

/// The characters that open the list without a letter typed: `ctx.` and the keys after `{` or `,`.
#[must_use]
pub fn is_trigger(ch: char) -> bool {
    matches!(ch, '.' | '{' | ',')
}

/// The items for the cursor at byte `offset` of `text`, sorted by label and without repeats.
/// Empty inside a string or a comment, where no completion applies, and where nothing is known.
#[must_use]
pub fn complete(text: &str, offset: usize, surface: &CtxSurface) -> Vec<Item> {
    let Some(before) = text.get(..offset) else {
        return Vec::new();
    };
    let scanned = scan(before);
    if scanned.lexical != Lexical::Code {
        return Vec::new();
    }
    let start = word_start(before);
    let word = &before[start..];
    let head = &before[..start];
    let mut items = match head.strip_suffix('.') {
        Some(chain_text) => dot_items(chain_text, surface),
        None => key_items(head, &scanned, surface),
    };
    items.retain(|item| item.label.starts_with(word));
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label);
    items
}

/// The items after a dot: `ctx.` lists the top level, `ctx.branch.` lists its calls.
fn dot_items(chain_text: &str, surface: &CtxSurface) -> Vec<Item> {
    let chain = chain_ending_at(chain_text, chain_text.len());
    let Some((first, path)) = chain.split_first() else {
        return Vec::new();
    };
    if *first != "ctx" {
        return Vec::new();
    }
    namespace_items(surface, &path.join("."))
}

/// The functions and groups directly under `prefix` (`""` for `ctx` itself). The helpers of
/// `ctx` are among the names at the top level, and get the text the reference gives them.
fn namespace_items(surface: &CtxSurface, prefix: &str) -> Vec<Item> {
    surface
        .names_under(prefix)
        .into_iter()
        .map(|name| {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            if prefix.is_empty()
                && let Some(helper) = helper_item(&name)
            {
                return helper;
            }
            match surface.get(&path) {
                Some(function) => function_item(name, function),
                None => Item {
                    label: name,
                    kind: ItemKind::Namespace,
                    detail: format!("ctx.{path}.*"),
                    documentation: format!(
                        "A group of calls. Type the dot after `ctx.{path}` to list them."
                    ),
                },
            }
        })
        .collect()
}

fn function_item(name: String, function: &CtxFn) -> Item {
    Item {
        label: name,
        kind: ItemKind::Function,
        detail: function.signature(),
        documentation: api_docs::function_summary(function),
    }
}

fn helper_item(name: &str) -> Option<Item> {
    let (call, documentation) = api_docs::helper_summary(name)?;
    Some(Item {
        label: name.to_owned(),
        kind: ItemKind::Function,
        detail: call,
        documentation,
    })
}

/// The keys of an argument table: the cursor is at a key of the table `{` that is the argument
/// of a call on `ctx.<path>`, after `{` or `,`. For a call with positional arguments, the table is
/// the options after them, so only the fields that are not positional are offered.
fn key_items(head: &str, scanned: &Scan, surface: &CtxSurface) -> Vec<Item> {
    let [.., (b'(', call_at), (b'{', brace_at)] = scanned.open.as_slice() else {
        return Vec::new();
    };
    if !matches!(head.trim_end().chars().last(), Some('{' | ',')) {
        return Vec::new();
    }
    let chain = chain_ending_at(&head[..*call_at], *call_at);
    let Some((first, path)) = chain.split_first() else {
        return Vec::new();
    };
    let Some(function) = (*first == "ctx")
        .then(|| surface.get(&path.join(".")))
        .flatten()
    else {
        return Vec::new();
    };
    let positional = function.positional();
    let options_index = match function.shape {
        ArgShape::Table => 0,
        ArgShape::Positional(names) => names.len(),
    };
    if top_level_commas(&head[*call_at + 1..*brace_at]) != options_index {
        return Vec::new();
    }
    function
        .fields
        .iter()
        .filter(|field| !positional.contains(&field.name))
        .map(|field| Item {
            label: field.name.to_owned(),
            kind: ItemKind::Field,
            detail: format!(
                "{}{}",
                api_docs::type_words(field.kind),
                if field.required { ", required" } else { "" }
            ),
            documentation: field.help.to_owned(),
        })
        .collect()
}

/// Whether the text ends inside a string or a comment, or in code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lexical {
    Code,
    Comment,
    Text,
}

/// How the text before the cursor ends: in code or not, and the brackets still open.
struct Scan {
    lexical: Lexical,
    /// Each open `(`, `{` or `[` with its byte offset, outermost first.
    open: Vec<(u8, usize)>,
}

/// Reads Lua from the start of `text` and says where the end is. Strings end at their quote or
/// at a newline, which a Lua string cannot cross unless escaped. Long brackets (`[==[ ... ]==]`)
/// make a long string or, after `--`, a long comment. A line comment runs to the newline.
fn scan(text: &str) -> Scan {
    let bytes = text.as_bytes();
    let mut open = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        match byte {
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                if let Some(level) = long_bracket_level(bytes, i) {
                    match close_of(bytes, i + level + 2, level) {
                        Some(end) => i = end,
                        None => {
                            return Scan {
                                lexical: Lexical::Comment,
                                open,
                            };
                        }
                    }
                } else {
                    match bytes[i..].iter().position(|&c| c == b'\n') {
                        Some(newline) => i += newline,
                        None => {
                            return Scan {
                                lexical: Lexical::Comment,
                                open,
                            };
                        }
                    }
                }
            }
            b'[' => {
                if let Some(level) = long_bracket_level(bytes, i) {
                    match close_of(bytes, i + level + 2, level) {
                        Some(end) => i = end,
                        None => {
                            return Scan {
                                lexical: Lexical::Text,
                                open,
                            };
                        }
                    }
                } else {
                    open.push((byte, i));
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                i += 1;
                loop {
                    match bytes.get(i) {
                        None => {
                            return Scan {
                                lexical: Lexical::Text,
                                open,
                            };
                        }
                        Some(b'\\') => i += 2,
                        Some(&c) if c == byte => {
                            i += 1;
                            break;
                        }
                        Some(b'\n') => break,
                        Some(_) => i += 1,
                    }
                }
            }
            b'(' | b'{' => {
                open.push((byte, i));
                i += 1;
            }
            b')' | b'}' | b']' => {
                open.pop();
                i += 1;
            }
            _ => i += 1,
        }
    }
    Scan {
        lexical: Lexical::Code,
        open,
    }
}

/// The level of a long bracket that starts at `i`: `[[` is 0, `[==[` is 2. `None` otherwise.
fn long_bracket_level(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'[') {
        return None;
    }
    let mut level = 0;
    let mut j = i + 1;
    while bytes.get(j) == Some(&b'=') {
        level += 1;
        j += 1;
    }
    (bytes.get(j) == Some(&b'[')).then_some(level)
}

/// The byte after the closing bracket `]==]` (for `level` 2) that ends a long bracket, searching
/// from `from`. `None` when it is not closed.
fn close_of(bytes: &[u8], from: usize, level: usize) -> Option<usize> {
    let close = format!("]{}]", "=".repeat(level));
    bytes
        .get(from..)?
        .windows(close.len())
        .position(|window| window == close.as_bytes())
        .map(|at| from + at + close.len())
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The byte offset where the identifier that ends `text` starts (`text.len()` when none does).
fn word_start(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut start = bytes.len();
    while start > 0 && is_word(bytes[start - 1]) {
        start -= 1;
    }
    start
}

/// The dotted names that end at byte `end` of `text`, such as `["ctx", "branch"]` for
/// `ctx.branch` ending there.
fn chain_ending_at(text: &str, end: usize) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut start = end;
    while start > 0 && (is_word(bytes[start - 1]) || bytes[start - 1] == b'.') {
        start -= 1;
    }
    text[start..end]
        .split('.')
        .filter(|name| !name.is_empty())
        .collect()
}

/// The commas of `text` outside any bracket: how many arguments came before the cursor's one.
/// Commas inside a string are counted too; a call that has one is not completed exactly.
fn top_level_commas(text: &str) -> usize {
    let mut depth: usize = 0;
    let mut commas = 0;
    for byte in text.bytes() {
        match byte {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    commas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> CtxSurface {
        CtxSurface::from_catalog()
    }

    /// The items at the end of `text`.
    fn at_end(text: &str) -> Vec<Item> {
        complete(text, text.len(), &surface())
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    /// A function of `ctx` that takes one table, and its field names.
    fn table_function() -> (&'static str, Vec<&'static str>) {
        let surface = surface();
        let function = surface
            .functions()
            .iter()
            .find(|f| f.shape == ArgShape::Table && !f.fields.is_empty())
            .expect("the catalog has a function with a table argument");
        (
            function.path,
            function.fields.iter().map(|f| f.name).collect(),
        )
    }

    #[test]
    fn after_ctx_dot_every_top_level_name_is_offered_once() {
        use gm_core::script::surface::HELPERS;
        let items = at_end("local x = ctx.");
        let names = labels(&items);
        for name in surface().names_under("") {
            assert!(names.contains(&name.as_str()), "{name} in {names:?}");
        }
        for helper in HELPERS {
            assert!(names.contains(helper), "{helper} in {names:?}");
        }
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(unique, names, "no repeats after sorting");
    }

    #[test]
    fn a_function_carries_its_signature_as_detail_and_its_reference_text_as_documentation() {
        let surface = surface();
        let exec = surface.get("exec").expect("exec is a function");
        let items = at_end("ctx.");
        let item = items
            .iter()
            .find(|item| item.label == "exec")
            .expect("exec is offered");
        assert_eq!(item.kind, ItemKind::Function);
        assert_eq!(item.detail, exec.signature());
        assert!(
            item.documentation.contains("Outcomes:"),
            "{}",
            item.documentation
        );
    }

    #[test]
    fn a_helper_is_a_function_with_its_call_and_text() {
        let items = at_end("ctx.");
        let now = items.iter().find(|item| item.label == "now").unwrap();
        assert_eq!(now.kind, ItemKind::Function);
        assert_eq!(now.detail, "ctx.now()");
        assert!(
            now.documentation.starts_with("Arguments: "),
            "{}",
            now.documentation
        );
    }

    #[test]
    fn a_dotted_action_makes_its_first_name_a_group() {
        let surface = surface();
        let dotted = surface
            .functions()
            .iter()
            .find(|f| f.path.contains('.'))
            .expect("the catalog has a dotted action");
        let group = dotted.path.split('.').next().unwrap();
        let items = at_end("ctx.");
        let item = items.iter().find(|item| item.label == group).unwrap();
        assert!(
            matches!(item.kind, ItemKind::Namespace | ItemKind::Function),
            "{group} is {:?}",
            item.kind
        );
    }

    #[test]
    fn the_prefix_filters_the_names() {
        let items = at_end("ctx.ex");
        assert!(!items.is_empty());
        assert!(
            items.iter().all(|item| item.label.starts_with("ex")),
            "{items:?}"
        );
        assert!(labels(&items).contains(&"exec"));
    }

    #[test]
    fn after_a_group_its_calls_are_offered_with_their_signatures() {
        let surface = surface();
        let calls = surface.names_under("branch");
        assert!(!calls.is_empty(), "the catalog has branch calls");
        let items = at_end("ctx.branch.");
        assert_eq!(
            labels(&items),
            calls.iter().map(String::as_str).collect::<Vec<_>>()
        );
        for item in &items {
            assert_eq!(item.kind, ItemKind::Function);
            assert_eq!(
                item.detail,
                surface
                    .get(&format!("branch.{}", item.label))
                    .unwrap()
                    .signature()
            );
        }
    }

    #[test]
    fn a_name_that_is_not_ctx_offers_nothing() {
        assert!(at_end("input.").is_empty());
        assert!(at_end("local ctx2 = 1\nctx2.").is_empty());
        assert!(at_end("ctx.nothing_here.").is_empty());
    }

    #[test]
    fn inside_a_string_nothing_is_offered() {
        assert!(at_end("local s = \"ctx.").is_empty());
        assert!(at_end("local s = 'ctx.").is_empty());
        assert!(
            at_end("local s = \"a\\\" ctx.").is_empty(),
            "an escaped quote does not close"
        );
    }

    #[test]
    fn a_string_ends_at_its_newline_and_completion_resumes() {
        assert!(!at_end("local s = \"abc\nctx.").is_empty());
    }

    #[test]
    fn inside_a_comment_nothing_is_offered() {
        assert!(at_end("-- ctx.").is_empty());
        assert!(at_end("--[[ ctx.").is_empty());
        assert!(at_end("--[==[ note\nctx.").is_empty());
    }

    #[test]
    fn after_a_comment_closes_completion_resumes() {
        assert!(!at_end("--[[ note ]] ctx.").is_empty());
        assert!(!at_end("-- note\nctx.").is_empty());
    }

    #[test]
    fn inside_a_long_string_nothing_is_offered() {
        assert!(at_end("local s = [[ ctx.").is_empty());
        assert!(at_end("local s = [=[ ctx. ]] ").is_empty());
        assert!(!at_end("local s = [=[ x ]=] ctx.").is_empty());
    }

    #[test]
    fn a_dash_inside_a_string_is_not_a_comment() {
        assert!(!at_end("local s = \"--\" ctx.").is_empty());
    }

    #[test]
    fn keys_of_a_table_argument_are_offered_after_the_brace() {
        let (path, fields) = table_function();
        let items = at_end(&format!("ctx.{path}({{ "));
        assert_eq!(labels(&items), {
            let mut sorted = fields.clone();
            sorted.sort_unstable();
            sorted
        });
        assert!(items.iter().all(|item| item.kind == ItemKind::Field));
    }

    #[test]
    fn keys_are_offered_after_a_comma_in_the_table_and_filtered_by_what_is_typed() {
        let (path, fields) = table_function();
        let first = fields[0];
        let typed = &first[..1];
        let items = at_end(&format!("ctx.{path}({{ {typed}"));
        assert!(labels(&items).contains(&first), "{first} in {items:?}");
        assert!(items.iter().all(|item| item.label.starts_with(typed)));
        let after_comma = at_end(&format!("ctx.{path}({{ {first} = \"x\", "));
        assert!(labels(&after_comma).contains(&first));
    }

    #[test]
    fn a_key_carries_its_type_and_its_help() {
        let (path, _) = table_function();
        let surface = surface();
        let function = surface.get(path).unwrap();
        let field = function.fields[0];
        let items = at_end(&format!("ctx.{path}({{ "));
        let item = items.iter().find(|item| item.label == field.name).unwrap();
        assert!(
            item.detail.starts_with(&api_docs::type_words(field.kind)),
            "{}",
            item.detail
        );
        assert_eq!(item.documentation, field.help);
        assert_eq!(item.detail.contains("required"), field.required);
    }

    #[test]
    fn no_key_is_offered_after_an_equals_sign() {
        let (path, _) = table_function();
        assert!(at_end(&format!("ctx.{path}({{ name = ")).is_empty());
    }

    #[test]
    fn no_key_is_offered_inside_a_nested_table() {
        let (path, _) = table_function();
        assert!(at_end(&format!("ctx.{path}({{ x = {{ ")).is_empty());
    }

    #[test]
    fn no_key_is_offered_outside_a_ctx_call() {
        assert!(at_end("local t = { ").is_empty());
        assert!(at_end("print({ ").is_empty());
    }

    #[test]
    fn with_positional_arguments_the_options_table_offers_only_the_other_fields() {
        let surface = surface();
        let exec = surface.get("exec").expect("exec");
        let positional = exec.positional();
        assert!(!positional.is_empty());
        let options: Vec<&str> = exec
            .fields
            .iter()
            .map(|f| f.name)
            .filter(|name| !positional.contains(name))
            .collect();
        assert!(!options.is_empty());
        let items = at_end("ctx.exec({ \"git\" }, { ");
        assert_eq!(labels(&items), {
            let mut sorted = options.clone();
            sorted.sort_unstable();
            sorted
        });
        for name in positional {
            assert!(!labels(&items).contains(name), "{name} is positional");
        }
    }

    #[test]
    fn the_first_positional_argument_is_not_a_table_of_keys() {
        assert!(at_end("ctx.exec({ ").is_empty());
    }

    #[test]
    fn a_comma_inside_a_nested_bracket_does_not_count_as_an_argument() {
        let surface = surface();
        let exec = surface.get("exec").unwrap();
        let options: Vec<&str> = exec
            .fields
            .iter()
            .map(|f| f.name)
            .filter(|name| !exec.positional().contains(name))
            .collect();
        let mut sorted = options.clone();
        sorted.sort_unstable();
        let items = at_end("ctx.exec(f(a, b), { ");
        assert_eq!(labels(&items), sorted);
    }

    #[test]
    fn the_offset_must_be_on_a_character_boundary() {
        assert!(complete("é ctx.", 1, &surface()).is_empty());
        assert!(!complete("é ctx.", "é ctx.".len(), &surface()).is_empty());
    }

    #[test]
    fn an_offset_past_the_text_offers_nothing() {
        assert!(complete("ctx.", 99, &surface()).is_empty());
    }

    #[test]
    fn the_cursor_in_the_middle_completes_the_word_before_it() {
        let text = "ctx.exec(argv)";
        let items = complete(text, "ctx.ex".len(), &surface());
        assert!(labels(&items).contains(&"exec"));
        assert!(items.iter().all(|item| item.label.starts_with("ex")));
    }

    #[test]
    fn the_triggers_are_the_dot_and_the_keys_openers() {
        assert!(is_trigger('.'));
        assert!(is_trigger('{'));
        assert!(is_trigger(','));
        assert!(!is_trigger('a'));
        assert!(!is_trigger('('));
        assert!(!is_trigger(' '));
    }

    #[test]
    fn the_scan_reports_the_open_brackets_outside_strings() {
        let scanned = scan("f({ \"}\" , [[ ]] ");
        assert_eq!(scanned.lexical, Lexical::Code);
        let kinds: Vec<u8> = scanned.open.iter().map(|(byte, _)| *byte).collect();
        assert_eq!(kinds, vec![b'(', b'{']);
    }

    #[test]
    fn top_level_commas_skip_the_brackets() {
        assert_eq!(top_level_commas("a, f(b, c), "), 2);
        assert_eq!(top_level_commas("{ a, b }, "), 1);
        assert_eq!(top_level_commas(""), 0);
    }

    #[test]
    fn items_are_sorted_by_label() {
        let items = at_end("ctx.");
        let names = labels(&items);
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }
}
