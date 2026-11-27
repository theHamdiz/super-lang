; --- comments ---------------------------------------------------------------
(doc_comment) @comment.documentation
(inner_doc_comment) @comment.documentation
(doc_block) @comment.documentation
(inner_doc_block) @comment.documentation
(comment) @comment

; Executable //# / /*# … #*/ — distinct from inert comments
(meta_comment) @attribute
(meta_comment) @keyword

; Hygienic quote / splice staging
(quote_expr) @punctuation.special
(splice_expr) @punctuation.special
(comptime_binding) @constant.macro
(goal_type) @function.builtin

; --- literals / values ------------------------------------------------------
(string) @string
(string_content) @string
(escape_sequence) @string.escape
(interpolation) @embedded

(number) @number

; true / false / none / it / index are keyword tokens — restyle by text
((keyword) @boolean
  (#match? @boolean "^(true|false)$"))

((keyword) @constant.builtin
  (#eq? @constant.builtin "none"))

((keyword) @variable.special
  (#match? @variable.special "^(it|index)$"))

((keyword) @type
  (#match? @type "^(type|Code)$"))

((keyword) @keyword
  (#match? @keyword "^(use|mod|test|state|when|ensure|else|for|each|while|loop|break|continue|in|return|fail|panic|or|err|if|await|scope|spawn|parallel|par|chan|shared|after|get|post|put|delete|rewrite|trait|embed)$"))

; Soft keywords after `.` win over the import/control keyword capture above
; (`@app.use` / `app.use` are methods, not `use` directives).
(
  (punctuation) @_dot
  (#eq? @_dot ".")
  .
  (keyword) @function.method
)

((keyword) @keyword.function
  (#match? @keyword.function "^(par|sum)$"))

((operator) @operator
  (#eq? @operator "=>"))

((operator) @operator.ternary
  (#eq? @operator "?"))

; --- types ------------------------------------------------------------------
(builtin_type) @type.builtin
(core_trait) @type.builtin

; ALL_CAPS before PascalCase so HTTP-style names stay constants
((ident) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

((ident) @type
  (#match? @type "^[A-Z][A-Za-z0-9_]*$"))

; --- label: value  (property before colon) ----------------------------------
;   host: "localhost"   {http: 80}   id: i64
(
  (ident) @property
  .
  ":"
)

; Typed field after colon
(
  ":"
  .
  [
    (builtin_type)
    (core_trait)
  ] @type.builtin
)

(
  ":"
  .
  (ident) @type
  (#match? @type "^[A-Z][A-Za-z0-9_]*$")
)

; --- visibility / mutation --------------------------------------------------
(export_marker) @keyword
(private_marker) @attribute
(mut_marker) @operator

; --- operators & delimiters -------------------------------------------------
(operator) @operator
(punctuation) @punctuation.delimiter
":" @punctuation.delimiter

; --- brackets = visual scope (works with Zed colorize_brackets) -------------
[
  "{"
  "}"
  "["
  "]"
  "("
  ")"
] @punctuation.bracket

; --- default identifiers (variables / funcs) --------------------------------
(ident) @variable
