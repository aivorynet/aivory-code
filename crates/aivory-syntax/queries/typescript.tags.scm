; Declarations of TypeScript and TSX. Each pattern has @definition.<kind> and @name.
; When two patterns match the same name, the earlier pattern wins.
(class_declaration name: (type_identifier) @name) @definition.class
(abstract_class_declaration name: (type_identifier) @name) @definition.class
(interface_declaration name: (type_identifier) @name) @definition.interface
(type_alias_declaration name: (type_identifier) @name) @definition.type
(enum_declaration name: (identifier) @name) @definition.enum
(enum_body (property_identifier) @name @definition.variant)
(enum_body (enum_assignment name: (property_identifier) @name) @definition.variant)
(internal_module name: [(identifier) (nested_identifier)] @name) @definition.namespace
(module name: (string) @name) @definition.namespace
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(function_signature name: (identifier) @name) @definition.function
(method_definition name: [(property_identifier) (private_property_identifier)] @name) @definition.method
(method_signature name: (property_identifier) @name) @definition.method
(abstract_method_signature name: (property_identifier) @name) @definition.method
(public_field_definition name: [(property_identifier) (private_property_identifier)] @name) @definition.field
(property_signature name: (property_identifier) @name) @definition.property
(program (lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @definition.function))
(program (export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @definition.function)))
(program (lexical_declaration "const" (variable_declarator name: (identifier) @name) @definition.constant))
(program (export_statement declaration: (lexical_declaration "const" (variable_declarator name: (identifier) @name) @definition.constant)))
(program (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program (export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)))
(program (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program (export_statement declaration: (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)))
