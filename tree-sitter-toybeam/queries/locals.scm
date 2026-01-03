; Scopes
(module) @local.scope
(function_definition) @local.scope
(block) @local.scope

; Definitions
(function_definition
  name: (identifier) @local.definition.function)

(parameter
  pattern: (identifier) @local.definition.parameter)

(let_statement
  pattern: (identifier) @local.definition.var)

; References
(identifier) @local.reference
