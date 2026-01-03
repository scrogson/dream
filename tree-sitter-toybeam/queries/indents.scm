; Indent
[
  (module)
  (function_definition)
  (struct_definition)
  (enum_definition)
  (block)
  (if_expression)
  (match_expression)
  (match_arm)
  (receive_expression)
  (tuple_expression)
  (list_expression)
  (struct_expression)
  (bitstring_expression)
] @indent

; Dedent
[
  "}"
  "]"
  ")"
  ">>"
] @indent.dedent

; Branch (for else)
(if_expression
  alternative: (_) @indent.branch)
