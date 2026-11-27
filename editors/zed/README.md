# Super extension for Zed

Syntax highlighting (Tree-sitter), outline, snippets, and **super-lsp** for `.sr` files.

## Language server

The extension starts `super lsp` (or your `lsp.super-lsp.binary.path`) and passes a stdlib root when it can find one:

1. `<workspace>/stdlib` (repo checkout)
2. `~/.cargo/share/super/stdlib` (after `cargo install --path crates/super-cli`)
3. Existing `SUPER_STDLIB` / `SUPER_STD` in the environment

You can force a path in Zed user settings:

```json
{
  "lsp": {
    "super-lsp": {
      "binary": {
        "path": "/path/to/super",
        "arguments": ["lsp"]
      }
    }
  }
}
```

Export `SUPER_STDLIB` before launching Zed if the workspace is not the Super repo.

## Stdlib autocomplete

With LSP connected, completions include:

- **`use std.…`** — all modules from `stdlib/mod.sr` (filtered as you type)
- **`+` exports** — public bindings, functions, and types from the current module and resolved imports
- **UFCS** — after `.` on a typed receiver, methods and public fields for that type

Trigger characters: `.` `@` `+` `#` `/`

## Install

From the repo:

```bash
./editors/zed/install.sh
# or: make install-zed
```

This builds `extension.wasm` + the tree-sitter grammar into a **clean `dist/`**
tree (no `Cargo.toml` / `target/`), then **symlinks**

`~/.local/share/zed/extensions/installed/super` → `editors/zed/dist`.

That symlink is what makes Zed treat Super as a **dev extension** (`dev: true`).
Redeploys only update files *inside* `dist/`. A content hash decides whether Zed
must reload the extension; identical reinstalls leave a live `super-lsp` alone.

## Hot reload (no full Zed restart)

After each `./editors/zed/install.sh` / `make install-zed`:

1. If wasm / grammar / queries changed → Zed reloads Super automatically.
2. Install then runs `zeditor --reuse <repo> <file.sr>` so **super-lsp
   respawns in the existing window** with the correct worktree (required on
   Linux, where CLI `-a`/`-e` only log `activate is not implemented`).
3. If content is unchanged → no reload, LSP stays up.

If you reload from the palette yourself (`zed: reload extensions`):

```bash
./editors/zed/respawn-lsp.sh
# or: command palette → editor: restart language server  (with a .sr buffer focused)
```

`zed: reload extensions` alone is not enough on Linux — Zed stops `super-lsp`
and will not restart it for already-open buffers until something opens a `.sr`
file in a reused/new workspace.

Do **not** use **`zed: rebuild dev extension`** for Super — `dist/` has no
Cargo crate on purpose. Always rebuild with `make install-zed`.

Quick sanity check:

```bash
readlink ~/.local/share/zed/extensions/installed/super
# → .../editors/zed/dist

python3 -c "import json;from pathlib import Path;e=json.load(open(Path.home()/'.local/share/zed/extensions/index.json'))['extensions']['super'];print(e['dev'], e['manifest']['version'])"
# → True 0.1.<epoch>

pgrep -af 'super lsp'
```

## Verify in Zed

1. Open the Super repo (or any project with `stdlib/` on disk or via env).
2. Confirm **Super** appears for `.sr` files and the language server is running (no “super binary not found” toast).
3. In a new `.sr` buffer, type `use std.h` and accept completion → `use std.net.http` (canonical manifest path).
4. Add `use std.http` (short alias) or `use std.net.http` — diagnostics/hover should resolve symbols; **Go to Definition** on the `use` line opens `stdlib/net/http.sr`.
