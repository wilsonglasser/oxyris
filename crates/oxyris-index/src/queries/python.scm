; Top-level symbols for Python.

(module
  (function_definition name: (identifier) @name) @function)

(module
  (class_definition name: (identifier) @name) @class)

; methods inside a class body
(class_definition
  body: (block
    (function_definition name: (identifier) @name) @method))

; module-level top-level constants: NAME = ...
(module
  (expression_statement
    (assignment
      left: (identifier) @name))) @constant

; decorated function/class — outer is decorated_definition; inner provides the name
(decorated_definition
  definition: (function_definition name: (identifier) @name)) @function

(decorated_definition
  definition: (class_definition name: (identifier) @name)) @class
