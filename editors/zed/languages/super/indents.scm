"{" @indent
"}" @outdent
"[" @indent
"]" @outdent
"(" @indent
")" @outdent

([
  (keyword) @indent
  (#match? @indent "^(loop|par|when|for|each|while|test)$")
]
  .
  "{")
