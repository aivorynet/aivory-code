(class name: [(constant) (scope_resolution)] @name) @definition.class
(module name: [(constant) (scope_resolution)] @name) @definition.module
(method name: (_) @name) @definition.function
(singleton_method name: (_) @name) @definition.function
(program (assignment left: (constant) @name) @definition.constant)
(class body: (body_statement (assignment left: (constant) @name) @definition.constant))
(module body: (body_statement (assignment left: (constant) @name) @definition.constant))
