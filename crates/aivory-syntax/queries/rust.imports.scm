(use_declaration
  argument: (use_as_clause path: (_) @path alias: (identifier) @alias)) @import
(use_declaration
  argument: [(identifier) (scoped_identifier) (scoped_use_list) (use_list) (use_wildcard)] @path) @import
(extern_crate_declaration name: (identifier) @path alias: (identifier)? @alias) @import
