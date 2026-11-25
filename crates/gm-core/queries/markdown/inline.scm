; Markdown, inline grammar (D260). Written for GitMaster after the query of
; tree-sitter-md 0.5.4 (MIT). It runs over the `inline` nodes of the block tree.

(code_span) @string

[
  (code_span_delimiter)
  (emphasis_delimiter)
] @punctuation.delimiter

(emphasis) @variable.parameter

(strong_emphasis) @constant

[
  (link_text)
  (image_description)
] @function

[
  (link_destination)
  (uri_autolink)
  (email_autolink)
] @constant

(link_label) @property

(link_title) @string

(backslash_escape) @escape

(inline_link
  [
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(image
  [
    "!"
    "["
    "]"
    "("
    ")"
  ] @punctuation.delimiter)

(shortcut_link
  [
    "["
    "]"
  ] @punctuation.delimiter)
