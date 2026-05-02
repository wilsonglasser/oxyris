; Top-level symbols for PHP.

(function_definition name: (name) @name) @function

(class_declaration name: (name) @name) @class

(interface_declaration name: (name) @name) @interface

(trait_declaration name: (name) @name) @trait

(enum_declaration name: (name) @name) @enum

(method_declaration name: (name) @name) @method

(namespace_definition name: (namespace_name) @name) @module

(const_declaration
  (const_element
    (name) @name)) @constant
