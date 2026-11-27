# super lint

`super lint` checks `.sr` files against named rules. Each rule has a stable
id, a name, and a default severity. Rules read the parsed module and its
tokens only, so a file can be linted without resolving its imports. Findings
print as `path:line:column: severity[ID name]: message`. The command exits 1
when any warning or error remains; `info` findings are reported but do not
fail the run.

```text
super lint                          # lint the current directory
super lint stdlib examples          # files or directories
super lint --fix examples           # apply the safe mechanical fixes in place
super lint --rules                  # print the rule table
super lint --config stdlib/lint.toml stdlib   # explicit policy for a tree that is not a Pod
```

## Rules

| Id | Name | Severity | Fix |
|----|------|----------|-----|
| S001 | `naming` | warning | — |
| S002 | `stutter` | warning | — |
| S003 | `unused` | warning | — |
| S004 | `shadowing` | warning | — |
| S005 | `missing-law` | info | — |
| S006 | `needless-mut` | warning | — |
| S007 | `redundant-plus` | warning | `--fix` |
| S008 | `redundant-pun` | warning | `--fix` |

**S001 `naming`.** Types, sum variants and traits are `UpperCamelCase`.
Functions, methods, parameters, fields, macros and `state` names are
`snake_case`; a leading `_` is allowed. Top-level bindings may be
`snake_case`, `UPPER_SNAKE` or `UpperCamelCase` (`+TaxRate = 0.14`).

**S002 `stutter`.** An exported name repeats its file's module name:
`orders_total` in `orders.sr`, or `OrdersCache` for a type. Entry files
(`main`, `lib`, `mod`, `macros`) are exempt.

**S003 `unused`.** A private item is never referenced in its module: a
top-level function or binding without `+`, a `-` field, or a `-` method.
`test` blocks, string interpolation (`"{NAME}"`) and the module's
`.law.sr` / `.proof.sr` companions count as references. `main`, names that
start with `_`, and functions that carry an attribute (`#get(…)`, `#law(…)`)
are exempt.

**S004 `shadowing`.** A construct that always introduces a new name reuses a
local or parameter of an enclosing scope: a closure parameter, a `for`
binding, a `when` payload binder, a destructuring name, or a `#for` name.
Plain `x = …` on an existing name is reassignment, not shadowing. `it`,
`index`, `err`, `self` and `_` are exempt.

**S005 `missing-law`.** A standard-library module that exports functions or
methods declares no `#law`, either in the module or in its `.law.sr`
companion. This rule only runs on files under `stdlib/` and is advisory.

**S006 `needless-mut`.** An `@` parameter or `@self` receiver is never
mutated in the body. A mutation is an assignment or compound assignment
rooted at the name, `@name` passed on (`@xs.push(1)`, `sink(@p)`), a
`for @name` loop, a call to a mutating built-in method (`push`, `pop`,
`insert`, `remove`, `clear`, `set`, …), or the name passed to a `host.*`
call, which acts on handles the linter cannot see into. A name that appears
inside a macro call counts as mutated. Dropping the `@` from a definition
does not break callers: passing `@x` to a plain parameter is still accepted.

**S007 `redundant-plus`.** A `+` in front of a struct field or method.
Members of a type are public by default; only top-level declarations take
`+`. The fix deletes the `+`.

**S008 `redundant-pun`.** `name: name` inside a struct literal (`User{id: id}`)
or inferred struct literal (`.{id: id}`) can be written `name`. The fix
rewrites it. Map literals are left alone, because `{id: id}` is a map and
`{id}` is a record.

## Configuration

A Pod configures the linter in `Pod.toml`. The nearest `Pod.toml` above each
file applies.

```toml
[lint]
allow = ["S004"]          # rules switched off (id or name)
deny = ["unused"]         # rules promoted to errors

[[lint.exception]]
rule = "stutter"          # id or name
path = "src/auth.sr"      # path suffix, relative to the Pod root
item = "AuthToken"        # optional: one named item
reason = "imported unqualified next to models.Task"
```

Every exception needs a non-empty `reason`. Unknown rule names are an error.

The standard library is not a Pod. Its policy lives in `stdlib/lint.toml`
(the same `[lint]` schema) and is passed with `--config`.

## Exceptions in this repository

- `stdlib/lint.toml`:
  - **S002**, 69 files and 916 names. Module-prefixed public names such as
    `json_parse` in `json.sr` are the established standard-library API, and
    callers also import them unqualified. Renaming them is a breaking change
    that is deferred to a deliberate API pass.
  - **S006**, 13 items:
    - `std.ai.mcp_client`, `std.io.console`, `std.net.tcp`, `std.net.udp`,
      and `std.net.http`'s `set_nonblock`. These are handle-backed I/O
      methods where `@self` marks the externally visible state change.
    - `std.net.http`'s `app_run_after`, where `@` passes the `App` by
      reference on the per-request path.
    - `std.core.collections`' `gd_lin_fit_i`, the kernel of frozen
      benchmark 20.
- `examples/02_task_api/Pod.toml`:
  - **S002** for `AuthToken`.
  - **S003** for the private `password_hash` field.
- **S005** reports 115 standard-library modules without laws. It is advisory
  (`info`) and is not suppressed.

CI runs `super lint --config stdlib/lint.toml stdlib` and
`super lint examples pods`. Both must report zero warnings and zero errors.
