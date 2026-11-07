(function_definition declarator: [
  (function_declarator declarator: (identifier) @name)
  (pointer_declarator declarator: (function_declarator declarator: (identifier) @name))
]) @definition.function
(translation_unit (declaration declarator: [
  (function_declarator declarator: (identifier) @name)
  (pointer_declarator declarator: (function_declarator declarator: (identifier) @name))
]) @definition.function)
(struct_specifier name: (type_identifier) @name body: (_)) @definition.struct
(union_specifier name: (type_identifier) @name body: (_)) @definition.struct
(enum_specifier name: (type_identifier) @name body: (_)) @definition.enum
(enumerator name: (identifier) @name) @definition.variant
(type_definition declarator: (type_identifier) @name) @definition.type
(preproc_def name: (identifier) @name) @definition.macro
(preproc_function_def name: (identifier) @name) @definition.macro
(field_declaration declarator: [
  (field_identifier) @name
  (pointer_declarator declarator: (field_identifier) @name)
  (array_declarator declarator: (field_identifier) @name)
]) @definition.field
(translation_unit (declaration declarator: [
  (identifier) @name
  (init_declarator declarator: (identifier) @name)
  (pointer_declarator declarator: (identifier) @name)
]) @definition.variable)
