(string) @string

(escape_sequence) @escape

; Keys come after the strings they are: the later pattern wins on the same range.
(pair
  key: (_) @string.special.key)

(number) @number

[
  (null)
  (true)
  (false)
] @constant.builtin

(comment) @comment
