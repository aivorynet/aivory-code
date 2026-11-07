; Imports of JavaScript, TypeScript, and TSX.
(import_statement source: (string) @path) @import
(import_statement (import_clause (identifier) @alias) source: (string) @path) @import
(import_statement (import_clause (namespace_import (identifier) @alias)) source: (string) @path) @import
(export_statement source: (string) @path) @import
(call_expression function: (identifier) @_f arguments: (arguments . (string) @path) (#eq? @_f "require")) @import
