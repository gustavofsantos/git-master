; Keys and tables
;-----------------

(quoted_key) @string

(table
  (bare_key) @type)

(table
  (dotted_key
    (bare_key) @type))

(table_array_element
  (bare_key) @type)

(table_array_element
  (dotted_key
    (bare_key) @type))

(pair
  (bare_key) @property)

(pair
  (dotted_key
    (bare_key) @property))

(pair
  (quoted_key) @property)

; Literals
;---------

(boolean) @boolean

(comment) @comment

(string) @string

[
  (integer)
  (float)
] @number

[
  (offset_date_time)
  (local_date_time)
  (local_date)
  (local_time)
] @string.special

; Punctuation
;------------

[
  "."
  ","
] @punctuation.delimiter

"=" @operator

[
  "["
  "]"
  "[["
  "]]"
  "{"
  "}"
] @punctuation.bracket
