; Top-level symbols for Rust.
; Capture names are mapped 1:1 to SymbolKind.

(function_item name: (identifier) @name) @function

(struct_item name: (type_identifier) @name) @struct

(enum_item name: (type_identifier) @name) @enum

(trait_item name: (type_identifier) @name) @trait

(mod_item name: (identifier) @name) @module

(const_item name: (identifier) @name) @constant

(static_item name: (identifier) @name) @constant

(type_item name: (type_identifier) @name) @type

(impl_item
  body: (declaration_list
    (function_item name: (identifier) @name) @method))

(macro_definition name: (identifier) @name) @function
