(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
(module (expression_statement (assignment left: (identifier) @name) @definition.variable))
(class_definition body: (block (expression_statement (assignment left: (identifier) @name) @definition.field)))
