; Declarations of Kotlin (the grammar ships no tags query). The first pattern wins on a name.
(package_header (qualified_identifier) @name) @definition.module
(class_declaration (modifiers (class_modifier) @_m) name: (identifier) @name (#eq? @_m "enum")) @definition.enum
(class_declaration "interface" name: (identifier) @name) @definition.interface
(class_declaration name: (identifier) @name) @definition.class
(object_declaration name: (identifier) @name) @definition.class
(companion_object name: (identifier) @name) @definition.class
(enum_entry (identifier) @name) @definition.variant
(function_declaration name: (identifier) @name) @definition.function
(property_declaration (variable_declaration (identifier) @name)) @definition.property
(type_alias type: (identifier) @name) @definition.type
