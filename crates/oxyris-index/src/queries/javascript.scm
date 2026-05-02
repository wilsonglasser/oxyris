; Top-level symbols for JavaScript / JSX.

(function_declaration name: (identifier) @name) @function

(class_declaration name: (identifier) @name) @class

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

(class_body
  (method_definition
    name: (property_identifier) @name) @method)
