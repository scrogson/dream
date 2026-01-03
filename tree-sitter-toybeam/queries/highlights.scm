; Keywords
[
  "mod"
  "fn"
  "let"
  "mut"
  "if"
  "else"
  "match"
  "struct"
  "enum"
  "spawn"
  "receive"
  "after"
  "return"
  "true"
  "false"
] @keyword

; Operators
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "&&"
  "||"
  "!"
  "="
  "->"
  "=>"
  "::"
  "|"
] @operator

; Binary syntax operators
[
  "<<"
  ">>"
] @operator

; Binary segment specifiers (these are inside segment_specifier node)
(segment_specifier) @keyword.modifier

; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
] @punctuation.delimiter

; Literals
(integer) @number
(string) @string
(escape_sequence) @string.escape
(atom) @string.special.symbol
(boolean) @boolean
(unit) @constant.builtin

; Types
(type_identifier) @type
(primitive_type) @type.builtin

; Functions
(function_definition
  name: (identifier) @function)

(call_expression
  function: (identifier) @function.call)

(method_call_expression
  method: (identifier) @function.method.call)

; Variables and parameters
(parameter
  pattern: (identifier) @variable.parameter)

(let_statement
  pattern: (identifier) @variable)

(identifier) @variable

; Struct and enum names
(struct_definition
  name: (type_identifier) @type.definition)

(enum_definition
  name: (type_identifier) @type.definition)

(enum_variant
  name: (type_identifier) @constructor)

(struct_expression
  name: (type_identifier) @type)

(struct_pattern
  name: (type_identifier) @type)

(enum_pattern
  type: (type_identifier) @type
  variant: (type_identifier) @constructor)

; Fields
(field_expression
  field: (identifier) @property)

(struct_field
  name: (identifier) @property)

(field_initializer
  name: (identifier) @property)

(field_pattern
  name: (identifier) @property)

; Module
(module
  name: (identifier) @module)

; Path expressions
(path_expression
  path: (type_identifier) @type)

; Comments
(line_comment) @comment
(block_comment) @comment

; Wildcards
(wildcard_pattern) @variable.builtin

; Visibility (pub keyword)
(visibility) @keyword.modifier
