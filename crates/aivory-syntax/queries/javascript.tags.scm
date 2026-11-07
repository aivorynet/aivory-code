; Declarations of JavaScript. Each pattern has @definition.<kind> and @name.
; When two patterns match the same name, the earlier pattern wins.
(class_declaration name: (identifier) @name) @definition.class
(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(method_definition name: [(property_identifier) (private_property_identifier)] @name) @definition.method
(field_definition property: [(property_identifier) (private_property_identifier)] @name) @definition.field
(program (lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @definition.function))
(program (export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name value: [(arrow_function) (function_expression)]) @definition.function)))
(program (lexical_declaration "const" (variable_declarator name: (identifier) @name) @definition.constant))
(program (export_statement declaration: (lexical_declaration "const" (variable_declarator name: (identifier) @name) @definition.constant)))
(program (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program (export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)))
(program (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program (export_statement declaration: (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)))
