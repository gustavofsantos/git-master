; Definitions only, for the symbol index (S-B). The grammar's own tags.scm with
; the reference patterns left out, and the associated types of impls and traits
; (`type Item = X;`, `type Output;`) left out too: they match the pattern for a
; type alias but are not worth finding by name.
(struct_item name: (type_identifier) @name) @definition.class
(enum_item name: (type_identifier) @name) @definition.class
(union_item name: (type_identifier) @name) @definition.class
(source_file (type_item name: (type_identifier) @name) @definition.class)
(mod_item body: (declaration_list (type_item name: (type_identifier) @name) @definition.class))
(declaration_list (function_item name: (identifier) @name) @definition.method)
(function_item name: (identifier) @name) @definition.function
(trait_item name: (type_identifier) @name) @definition.interface
(mod_item name: (identifier) @name) @definition.module
(macro_definition name: (identifier) @name) @definition.macro
