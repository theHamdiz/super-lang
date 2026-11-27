; Token grammar — coarse outline; LSP fills detail when available.
(export_marker) @name @item

((keyword) @name @item
  (#eq? @name "test"))

((keyword) @name @item
  (#match? @name "^(state|use|mod)$"))
