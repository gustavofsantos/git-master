; Markdown, block grammar (D260). Written for GitMaster after the query of
; tree-sitter-md 0.5.4 (MIT). The inline text is colored by inline.scm.

[
  (atx_heading)
  (setext_heading)
] @keyword

[
  (fenced_code_block)
  (indented_code_block)
] @string

(fenced_code_block_delimiter) @punctuation.delimiter

(info_string) @attribute

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
  (block_quote_marker)
] @punctuation.special

(link_destination) @constant

(link_label) @property

(link_title) @string

(backslash_escape) @escape
