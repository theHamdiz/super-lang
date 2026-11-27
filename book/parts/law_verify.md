# Law-Guarded Verification & AI-Safe Workflows

Super keeps a fixed 45-keyword surface. Formal contracts ride the same `#`
meta vocabulary as `#route` and `#assert_size`:

```super
#law("transfer preserves total balance across all valid amounts")
transfer_conserves_total(src: Account, dst: Account, amount: i64) {
    ensure amount >= 0 && src.balance >= amount
    ensure dst.balance <= i64.MAX - amount
    s = src
    d = dst
    ok = transfer(@s, @d, amount) or err {
        #unreachable("preconditions guarantee transfer succeeds")
    }
    #assert(ok && (s.balance + d.balance == src.balance + dst.balance))
}
```

## Bend-style file separation

| File | Owner | Contents |
|---|---|---|
| `*.law.sr` | Humans | Locked behavioral invariants (`#law`) |
| `*.proof.sr` | Agents | Lemmas, witnesses, equational `#assert`s (`#proof`) |

`super verify --locked-laws` (CI) fails if a `#law` body drifts from `laws.lock`.
Agents heal implementations from **E0502** counterexamples and never edit
`*.law.sr`.

## Three tiers, zero runtime

1. **SMT / bitvector** — pure `i64` (`-fwrapv`), bools, struct fields, `@` borrows
2. **Comptime rewrite** — algebraic identities and reducer laws
3. **Bounded fuzz** — deterministic property harnesses for residual cases

After discharge, `#law` and `#proof` are erased the same way release profiles
strip `test` blocks: **0 bytes** in C23 AOT, JIT, and `.superb`.

```bash
super verify                 # or: super v
super verify --solver smt
super verify --locked-laws --json
```

Pod.toml:

```toml
[verify]
require-on-release = false
# solver = "auto"
```

MCP agents call `super_verify` and read `super://laws`. See `examples/04_law_verify/`
and `SPECIFICATION.md` §3.9 / §7.1 / §9.2 / §10.
