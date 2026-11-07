; Declarations of C++, in addition to c.tags.scm (the Rust code joins the two files).
(function_definition declarator: [
  (function_declarator declarator: (field_identifier) @name)
  (function_declarator declarator: (qualified_identifier name: [(identifier) (destructor_name) (operator_name)] @name))
  (function_declarator declarator: (destructor_name) @name)
  (_ (function_declarator declarator: [(identifier) (field_identifier)] @name))
  (_ (function_declarator declarator: (qualified_identifier name: (identifier) @name)))
]) @definition.function
(field_declaration declarator: (function_declarator declarator: (field_identifier) @name)) @definition.method
(declaration_list (declaration declarator: (function_declarator declarator: (identifier) @name)) @definition.function)
(class_specifier name: (type_identifier) @name body: (_)) @definition.class
(namespace_definition name: (namespace_identifier) @name) @definition.namespace
(alias_declaration name: (type_identifier) @name) @definition.type
(concept_definition name: (identifier) @name) @definition.type
