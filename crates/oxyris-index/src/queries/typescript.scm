; Top-level symbols for TypeScript / TSX.

(function_declaration name: (identifier) @name) @function

(class_declaration name: (type_identifier) @name) @class

(interface_declaration name: (type_identifier) @name) @interface

(type_alias_declaration name: (type_identifier) @name) @type

(enum_declaration name: (identifier) @name) @enum

; const/let/var foo = (...) => ...
(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    value: (arrow_function))) @function

(variable_declaration
  (variable_declarator
    name: (identifier) @name
    value: (arrow_function))) @function

; const/let/var foo = function () { ... }
(lexical_declaration
  (variable_declarator
    name: (identifier) @name
    value: (function_expression))) @function

; class methods
(class_body
  (method_definition
    name: (property_identifier) @name) @method)

; module declarations (namespace foo { })
(internal_module
  name: (identifier) @name) @module

; exported re-statements still produce captures because we walk the inner node.
