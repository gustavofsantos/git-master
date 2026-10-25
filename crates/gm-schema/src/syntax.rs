use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a piece of source code is, for coloring (N4, D111). A theme gives each
/// role one color; text with no role keeps the plain text color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxRole {
    Comment,
    String,
    Number,
    Keyword,
    Operator,
    Function,
    Type,
    Constant,
    /// A parameter, or a variable the language gives a meaning (`self`, `this`).
    Variable,
    Property,
    Punctuation,
    /// An attribute, a macro, a decorator, a label or a tag.
    Attribute,
}

/// A byte range of a file's text and its role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub role: SyntaxRole,
}
