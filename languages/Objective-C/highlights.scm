; The original Source/Author https://github.com/blacktop/zed-objc
; MIT License

; Copyright (c) 2025 blacktop

; Permission is hereby granted, free of charge, to any person obtaining a copy
; of this software and associated documentation files (the "Software"), to deal
; in the Software without restriction, including without limitation the rights
; to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
; copies of the Software, and to permit persons to whom the Software is
; furnished to do so, subject to the following conditions:

; The above copyright notice and this permission notice shall be included in all
; copies or substantial portions of the Software.

; THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
; IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
; FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
; AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
; LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
; OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
; SOFTWARE.

; Rewritten for Zed. Zed rule: for the same range, the LAST matching pattern
; wins. So broad captures come first and specific ones come later.
; Supported predicates only: #eq? #match? #any-of? (never #has-ancestor? etc.).

; ---------------------------------------------------------------------------
; Keywords (C + control flow)
; ---------------------------------------------------------------------------

[
  "const"
  "enum"
  "extern"
  "inline"
  "sizeof"
  "static"
  "struct"
  "typedef"
  "union"
  "volatile"
  "break"
  "case"
  "continue"
  "default"
  "do"
  "else"
  "for"
  "goto"
  "if"
  "return"
  "switch"
  "while"
  "__typeof__"
  "__typeof"
  "typeof"
  "in"
  "oneway"
] @keyword

; Preprocessor directive tokens

[
  "#define"
  "#elif"
  "#else"
  "#endif"
  "#if"
  "#ifdef"
  "#ifndef"
  "#include"
  "#import"
  (preproc_directive)
] @keyword

; Objective-C @-directives

[
  "@interface"
  "@implementation"
  "@protocol"
  "@end"
  "@property"
  "@synthesize"
  "@dynamic"
  "@selector"
  "@autoreleasepool"
  "@synchronized"
  "@try"
  "@catch"
  "@finally"
  "@throw"
  "@import"
  "@available"
  "@optional"
  "@required"
  "@compatibility_alias"
  "@defs"
  "availability"
  "__builtin_available"
  "__covariant"
  "__contravariant"
  (visibility_specification)
  (protocol_qualifier)
] @keyword

(class_declaration "@" @keyword "class" @keyword)

; ---------------------------------------------------------------------------
; Operators & punctuation
; ---------------------------------------------------------------------------

[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "&="
  "|="
  "^="
  "<<="
  ">>="
  "++"
  "--"
  "+"
  "-"
  "*"
  "/"
  "%"
  "~"
  "&"
  "|"
  "^"
  "<<"
  ">>"
  "!"
  "&&"
  "||"
  "=="
  "!="
  "<"
  ">"
  "<="
  ">="
  "->"
  "?"
  ":"
] @operator

[
  "."
  ";"
  ","
] @punctuation.delimiter

[
  "{"
  "}"
  "("
  ")"
  "["
  "]"
] @punctuation.bracket

"@" @punctuation.special

; Method +/- sign (after operators so it wins over the "+"/"-" operator tokens)

(method_definition ["+" "-"] @keyword)
(method_declaration ["+" "-"] @keyword)

; ---------------------------------------------------------------------------
; Literals
; ---------------------------------------------------------------------------

[
  (string_literal)
  (system_lib_string)
  (char_literal)
] @string

(comment) @comment

(number_literal) @number

[
  (true)
  (false)
] @boolean

(null) @constant.builtin

; ---------------------------------------------------------------------------
; Built-in types (C primitive type keywords + Cocoa builtin types)
; ---------------------------------------------------------------------------

[
  (primitive_type)
  (sized_type_specifier)
] @keyword

[
  "BOOL"
  "IMP"
  "SEL"
  "Class"
  "id"
] @type.builtin

; ---------------------------------------------------------------------------
; Identifiers (ORDER MATTERS — last match wins)
; broad @variable first, then @function, then @constant, then self/super.
; ---------------------------------------------------------------------------

(identifier) @variable

(field_identifier) @property

; Objective-C ivars (project convention: _snake_case) -> property color, like Xcode.
; Comes BEFORE the selector/method patterns so underscore-prefixed *selectors*
; (e.g. [self _privateMethod]) still win as @function below (last match wins).
((identifier) @property
  (#match? @property "^_[a-z]"))

; Functions, C calls, message selectors, method names
(call_expression
  function: (identifier) @function)
(call_expression
  function: (field_expression
    field: (field_identifier) @function))
(function_declarator
  declarator: (identifier) @function)
(message_expression
  method: (identifier) @function)
(method_definition (identifier) @function)
(method_declaration (identifier) @function)
(method_identifier (identifier) @function)

; ALL_CAPS identifiers -> constant (enum consts; LSP refines real macros to green)
((identifier) @constant
  (#match? @constant "^_*[A-Z][A-Z\\d_]*$"))

; self / super -> keyword
((identifier) @keyword
  (#any-of? @keyword "self" "super"))

; ---------------------------------------------------------------------------
; Types, classes, protocols (identifier-based names come after @variable so
; they win)
; ---------------------------------------------------------------------------

(type_identifier) @type

((type_identifier) @type.builtin
  (#eq? @type.builtin "instancetype"))

(class_declaration (identifier) @type)

(class_interface "@interface" . (identifier) @type superclass: _? @type category: _? @type)

(class_implementation "@implementation" . (identifier) @type superclass: _? @type category: _? @type)

(protocol_forward_declaration (identifier) @type)

(protocol_reference_list (identifier) @type)

; ---------------------------------------------------------------------------
; Preprocessor macro names -> constant (LSP refines real macros to green)
; ---------------------------------------------------------------------------

(preproc_def
  name: (identifier) @constant)
(preproc_function_def
  name: (identifier) @constant)
(preproc_undef
  name: (_) @constant)
