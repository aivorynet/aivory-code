; Highlights of Kotlin (the grammar ships no highlights query).
[(line_comment) (block_comment)] @comment
[(string_literal) (multiline_string_literal) (character_literal)] @string
(escape_sequence) @string.escape
[(number_literal) (float_literal)] @number
(annotation) @attribute
(type_alias type: (identifier) @type)
(user_type (identifier) @type)
(class_declaration name: (identifier) @type)
(object_declaration name: (identifier) @type)
(function_declaration name: (identifier) @function)
(call_expression . (identifier) @function)
(parameter (identifier) @variable.parameter)
[(visibility_modifier) (class_modifier) (function_modifier) (member_modifier) (inheritance_modifier) (property_modifier) (parameter_modifier) (platform_modifier)] @keyword
["fun" "val" "var" "class" "interface" "object" "if" "else" "when" "for" "while" "do" "return" "try" "catch" "finally" "throw" "import" "package" "is" "in" "as" "typealias" "constructor" "init" "this" "super" "by" "companion" "enum" "!is" "!in" "as?" "where" "get" "set"] @keyword
["(" ")" "[" "]" "{" "}" "," ";" "." ":" "::"] @punctuation.delimiter
["+" "-" "*" "/" "%" "=" "==" "!=" "<" ">" "<=" ">=" "&&" "||" "!" "->" "?" "?:" "?." ".." "+=" "-=" "*=" "/=" "++" "--"] @operator
