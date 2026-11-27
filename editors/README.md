# Super editor integrations

All production editors should use **`super lsp`** (stdio JSON-RPC) for IntelliSense — not static JSON completion lists.

## Shared environment

Set the stdlib root so `use std.*` resolves to your tree (workspace or install):

```bash
export SUPER_STDLIB=/path/to/super/stdlib   # or SUPER_STD (same meaning)
```

Resolution order in the compiler/LSP matches `crates/super-frontend/src/modules.rs`:

1. `SUPER_STDLIB` / `SUPER_STD`
2. Installed toolchain (`share/super/stdlib` next to `super`)
3. Walk upward from the open file for `stdlib/mod.sr`
4. Embedded stdlib snapshot

Canonical import completions use manifest paths: `use std.net.http`, `use std.core.str`, `use std.io.socket`, etc. Short aliases (`use std.http`) still resolve at compile time but completions prefer the full path.

Documentation comments are `///` and `/** … */` on the next declaration, and `//!` and `/*! … */` on the enclosing module or type. The TextMate grammars (VS Code and the TextMate bundle) and the Tree-sitter grammar (Zed, Neovim, Helix) highlight them as documentation, including `` `code` ``, `**bold**`, and `# headings` inside the comment. Hover, completion, signature help, and `super doc` read the same comments from the parser.

## LSP features (`crates/super-lsp`)

| Feature | Notes |
|--------|--------|
| Completion | Keywords, `use std.…` from live `stdlib/mod.sr`, locals, `+` exports, UFCS after `.` |
| Hover | Types, structs (including `: Trait` on the header), trait signatures, UFCS methods, `///` / `//!` / `/**` documentation, comptime `#`, keyword/sigil help |
| Go to definition | Locals + cross-file stdlib/sibling modules |
| References / rename | Symbol index from sema |
| Diagnostics | Project `use` graph (same as `super check`) |
| Semantic tokens | Exports, mutability, imports, parameters |
| Signature help / inlay hints / document symbol | Enabled in `initialize` |

Run the server: `super lsp` (binary from `crates/super-cli`).

## Per-editor

| Editor | Path | Status |
|--------|------|--------|
| VS Code | `editors/vscode/` | Language Client, `SUPER_STDLIB` injection, semantic highlighting default |
| Zed | `editors/zed/` | WASM extension, `super-lsp` + stdlib env |
| Tree-sitter | `editors/tree-sitter-super/` | Grammar/highlights; LSP provides semantics |
| TextMate bundle | `editors/textmate/` | Legacy syntax only — pair with any LSP client using `super lsp` |
| NeoVim / Helix | — | Use `super lsp` via your LSP config; set `SUPER_STDLIB` in the server `cmd` env |

### VS Code

```bash
cd editors/vscode && npm install && npm run compile
# Install VSIX or run Extension Development Host from this folder
```

Settings: `super.languageServerPath`, `super.languageServerArgs` (default `["lsp"]`).

### Zed

See [zed/README.md](zed/README.md). Build/install with `editors/zed/install.sh`.

### Tree-sitter

```bash
cd editors/tree-sitter-super && npx tree-sitter generate && npx tree-sitter build --wasm
```

Zed consumes `grammars/super.wasm`; VS Code uses TextMate unless you wire tree-sitter separately.