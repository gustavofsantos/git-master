; Highlights of Lua, for the file viewer and the Script tab of the Workflows page (D233). Written
; for GitMaster; workflows run as Luau, so two of its words are colored too. Where two rules
; overlap, the later one wins.

; Keywords. `continue` and `export` are Luau only: the Lua grammar reads them as identifiers, so
; they are matched by their text. `type` is not a keyword here, because it is a Lua builtin too.
[
  "and"
  "do"
  "else"
  "elseif"
  "end"
  "for"
  "function"
  "goto"
  "if"
  "in"
  "local"
  "not"
  "or"
  "repeat"
  "return"
  "then"
  "until"
  "while"
] @keyword

(break_statement) @keyword

((identifier) @keyword
  (#any-of? @keyword "continue" "export"))

[
  (true)
  (false)
] @boolean

(nil) @constant

(number) @number
(string) @string
(escape_sequence) @string.escape
(comment) @comment

[
  ","
  ";"
  "."
  ":"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(binary_expression
  operator: _ @operator)

(unary_expression
  operator: _ @operator)

; Names of fields and of keys in a table, such as `name` in `{ name = "x" }` and `create` in
; `ctx.branch.create`.
(field
  name: (identifier) @property)

(dot_index_expression
  field: (identifier) @property)

; Functions, where they are declared and where they are called. Calls come last, so a called
; `ctx.branch.create` is a function, not a property.
(function_declaration
  name: (identifier) @function)

(function_declaration
  name: (dot_index_expression
    field: (identifier) @function))

(function_declaration
  name: (method_index_expression
    method: (identifier) @function))

(function_call
  name: (identifier) @function)

(function_call
  name: (dot_index_expression
    field: (identifier) @function))

(function_call
  name: (method_index_expression
    method: (identifier) @function))

; The context object of a workflow. It is painted like `self` in other languages.
((identifier) @variable.builtin
  (#eq? @variable.builtin "ctx"))
