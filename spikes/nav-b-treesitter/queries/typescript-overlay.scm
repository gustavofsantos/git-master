; What neither the JavaScript nor the TypeScript tags query captures (S-B).
(enum_declaration
  name: (identifier) @name) @definition.class

(type_alias_declaration
  name: (type_identifier) @name) @definition.type

(public_field_definition
  name: (property_identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function
