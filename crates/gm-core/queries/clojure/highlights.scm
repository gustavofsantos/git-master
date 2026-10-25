;; Literals

(num_lit) @number

[
  (char_lit)
  (str_lit)
  (regex_lit)
] @string

[
  (bool_lit)
  (nil_lit)
] @constant.builtin

(kwd_lit) @constant

;; Comments: a line comment, and the #_ that discards the next form (the form keeps its colors)

(comment) @comment

(dis_expr) @comment

;; Metadata (^:private, ^String) and tags (#inst)

(meta_lit) @attribute
(old_meta_lit) @attribute

; What the metadata holds is smaller than the metadata, so it needs its own capture
(meta_lit
  (kwd_lit) @attribute)

(meta_lit
  (sym_lit
    (sym_name) @attribute))

;; The namespace part of a qualified symbol (clojure.string/join)

(sym_ns) @type

;; The head of a list is the thing being called

(list_lit
  .
  (sym_lit
    (sym_name) @function))

;; Special forms and the core macros that read as keywords

(list_lit
  .
  (sym_lit
    (sym_name) @keyword)
  (#match? @keyword "^(def|defn|defn-|defmacro|defmulti|defmethod|defonce|defprotocol|defrecord|deftype|definterface|definline|defstruct|ns|in-ns|fn|fn\\*|let|let\\*|letfn|loop|recur|do|if|if-not|if-let|if-some|when|when-not|when-let|when-some|when-first|cond|condp|cond->|cond->>|case|try|catch|finally|throw|quote|var|set!|binding|for|doseq|dotimes|while|and|or|not|->|->>|as->|some->|some->>|doto|new|import|require|use|refer|comment|declare|defn\\*|reify|proxy|extend-type|extend-protocol|lazy-seq|delay|future|locking|monitor-enter|monitor-exit)$"))

;; The name a definition introduces: a function, a type or a value

(list_lit
  .
  (sym_lit
    (sym_name) @_head)
  .
  (sym_lit
    (sym_name) @function)
  (#match? @_head "^(defn|defn-|defmacro|defmulti|defmethod|definline)$"))

(list_lit
  .
  (sym_lit
    (sym_name) @_head)
  .
  (sym_lit
    (sym_name) @type)
  (#match? @_head "^(defrecord|deftype|defprotocol|definterface|defstruct|ns|in-ns)$"))

(list_lit
  .
  (sym_lit
    (sym_name) @_head)
  .
  (sym_lit
    (sym_name) @constant)
  (#match? @_head "^(def|defonce|declare)$"))

;; Quoting and dereferencing read as operators

[
  "'"
  "`"
  "~"
  "@"
  "~@"
  "#'"
] @operator

;; Brackets

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket
