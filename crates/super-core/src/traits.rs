//! Language traits. The same table drives semantic checks, LSP hover, and the
//! runtime witness. A standard-library definition that drifts from this table
//! is a compiler bug (`E0204`), not a new trait.

/// One method of a language trait.
///
/// `params` counts every parameter, including `self` when `has_self` is set.
/// `ret` is required only when the language fixes the return type; a source
/// method may be more specific when `ret` is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreMethod {
    pub name: &'static str,
    pub params: u8,
    pub has_self: bool,
    pub mutable_self: bool,
    pub ret: Option<&'static str>,
}

/// A language trait and the methods that witness it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreTrait {
    pub name: &'static str,
    pub methods: &'static [CoreMethod],
}

const fn method(
    name: &'static str,
    params: u8,
    has_self: bool,
    mutable_self: bool,
    ret: Option<&'static str>,
) -> CoreMethod {
    CoreMethod {
        name,
        params,
        has_self,
        mutable_self,
        ret,
    }
}

/// a systems peer's core operator and conversion traits, in Super's shape.
///
/// `Eq` covers both `PartialEq` and `Eq`. `Ord` covers `PartialOrd` and `Ord`.
/// `Copy` is a marker. There is no separate `impl` block: a struct names the
/// trait in its header and writes the methods in its body.
pub static CORE_TRAITS: &[CoreTrait] = &[
    CoreTrait {
        name: "Display",
        methods: &[method("display", 1, true, false, Some("str"))],
    },
    CoreTrait {
        name: "Debug",
        methods: &[method("debug", 1, true, false, Some("str"))],
    },
    CoreTrait {
        name: "ToString",
        methods: &[method("to_str", 1, true, false, Some("str"))],
    },
    CoreTrait {
        name: "Eq",
        methods: &[method("eq", 2, true, false, Some("bool"))],
    },
    CoreTrait {
        name: "Ord",
        methods: &[method("cmp", 2, true, false, Some("i64"))],
    },
    CoreTrait {
        name: "Hash",
        methods: &[method("hash", 1, true, false, Some("i64"))],
    },
    CoreTrait {
        name: "Clone",
        methods: &[method("clone", 1, true, false, None)],
    },
    CoreTrait {
        name: "Copy",
        methods: &[],
    },
    CoreTrait {
        name: "Default",
        methods: &[method("default", 0, false, false, None)],
    },
    CoreTrait {
        name: "From",
        methods: &[method("from", 1, false, false, None)],
    },
    CoreTrait {
        name: "Into",
        methods: &[method("into", 1, true, false, None)],
    },
    CoreTrait {
        name: "AsRef",
        methods: &[method("as_ref", 1, true, false, None)],
    },
    CoreTrait {
        name: "Borrow",
        methods: &[method("borrow", 1, true, false, None)],
    },
    CoreTrait {
        name: "Iter",
        methods: &[method("next", 1, true, true, None)],
    },
    CoreTrait {
        name: "Add",
        methods: &[method("add", 2, true, false, None)],
    },
    CoreTrait {
        name: "Sub",
        methods: &[method("sub", 2, true, false, None)],
    },
    CoreTrait {
        name: "Mul",
        methods: &[method("mul", 2, true, false, None)],
    },
    CoreTrait {
        name: "Div",
        methods: &[method("div", 2, true, false, None)],
    },
    CoreTrait {
        name: "Rem",
        methods: &[method("rem", 2, true, false, None)],
    },
    CoreTrait {
        name: "Neg",
        methods: &[method("neg", 1, true, false, None)],
    },
    CoreTrait {
        name: "Not",
        methods: &[method("not", 1, true, false, None)],
    },
    CoreTrait {
        name: "Index",
        methods: &[method("index", 2, true, false, None)],
    },
    CoreTrait {
        name: "Error",
        methods: &[method("message", 1, true, false, Some("str"))],
    },
    CoreTrait {
        name: "Drop",
        methods: &[method("drop", 1, true, true, None)],
    },
    // Associative monoid used by sequential and parallel reductions.
    CoreTrait {
        name: "AssociativeReducer",
        methods: &[
            method("identity", 0, false, false, None),
            method("accumulate", 2, true, true, None),
            method("merge", 2, true, true, None),
            method("finalize", 1, true, false, None),
        ],
    },
    // Zero-allocation iteration adapters — fused pipeline lowering in the compiler.
    CoreTrait {
        name: "Iterable",
        methods: &[
            method("map", 2, true, false, None),
            method("keep", 2, true, false, None),
            method("reduce", 2, true, false, None),
            method("sum", 1, true, false, None),
            method("product", 1, true, false, None),
            method("min", 1, true, false, None),
            method("max", 1, true, false, None),
            method("any", 1, true, false, None),
            method("all", 1, true, false, None),
            method("mean", 1, true, false, None),
            method("variance", 1, true, false, None),
        ],
    },
    // Parallel counterpart of `Iterable` (OpenMP / par chunking when pure).
    CoreTrait {
        name: "ParIterable",
        methods: &[
            method("par_reduce", 2, true, false, None),
            method("par_sum", 1, true, false, None),
            method("par_product", 1, true, false, None),
            method("par_min", 1, true, false, None),
            method("par_max", 1, true, false, None),
            method("par_mean", 1, true, false, None),
            method("par_variance", 1, true, false, None),
        ],
    },
];

/// Language traits implemented by a builtin type. User structs opt in by name.
pub fn builtin_traits(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "i8" | "i16" | "i32" | "i64" | "i128" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => &[
            "Display", "Debug", "Eq", "Ord", "Hash", "Clone", "Copy", "Default", "Add", "Sub",
            "Mul", "Div", "Rem", "Neg",
        ],
        "bool" => &[
            "Display", "Debug", "Eq", "Ord", "Hash", "Clone", "Copy", "Default", "Not",
        ],
        "str" => &[
            "Display", "Debug", "ToString", "Eq", "Ord", "Hash", "Clone", "Copy", "Default", "Add",
            "Index", "AsRef", "Borrow",
        ],
        "array" => &[
            "Display",
            "Debug",
            "Eq",
            "Ord",
            "Hash",
            "Clone",
            "Default",
            "Add",
            "Index",
            "Iter",
            "Iterable",
            "ParIterable",
        ],
        "map" | "struct" => &[
            "Display", "Debug", "Eq", "Hash", "Clone", "Default", "Index",
        ],
        "range" => &[
            "Display",
            "Debug",
            "Eq",
            "Clone",
            "Iter",
            "Iterable",
            "ParIterable",
        ],
        "error" => &["Display", "Debug", "Eq", "Error"],
        _ => &[],
    }
}

pub fn core_trait(name: &str) -> Option<&'static CoreTrait> {
    CORE_TRAITS.iter().find(|tr| tr.name == name)
}

pub fn core_method(method: &str) -> Option<(&'static CoreTrait, &'static CoreMethod)> {
    CORE_TRAITS.iter().find_map(|tr| {
        tr.methods
            .iter()
            .find(|m| m.name == method)
            .map(|m| (tr, m))
    })
}

/// Intern every language-trait and method name so later lookups agree.
pub fn intern_core(interner: &mut crate::Interner) {
    for tr in CORE_TRAITS {
        interner.intern(tr.name);
        for method in tr.methods {
            interner.intern(method.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique_and_cover_rust_core() {
        let mut names = std::collections::BTreeSet::new();
        for tr in CORE_TRAITS {
            assert!(names.insert(tr.name), "duplicate trait {}", tr.name);
            let mut methods = std::collections::BTreeSet::new();
            for method in tr.methods {
                assert!(
                    methods.insert(method.name),
                    "{} repeats {}",
                    tr.name,
                    method.name
                );
                if method.has_self {
                    assert!(method.params >= 1);
                }
                if method.mutable_self {
                    assert!(method.has_self);
                }
            }
        }
        for required in [
            "Display",
            "Debug",
            "Eq",
            "Ord",
            "Hash",
            "Clone",
            "Copy",
            "Default",
            "From",
            "Into",
            "AsRef",
            "Iter",
            "Add",
            "Drop",
            "Error",
            "AssociativeReducer",
            "Iterable",
            "ParIterable",
        ] {
            assert!(core_trait(required).is_some(), "missing {required}");
        }
    }

    #[test]
    fn builtin_numbers_implement_the_arithmetic_witness() {
        let traits = builtin_traits("i64");
        for name in ["Display", "Eq", "Ord", "Hash", "Add", "Copy"] {
            assert!(traits.contains(&name), "i64 missing {name}");
        }
        assert!(builtin_traits("str").contains(&"Display"));
        assert!(builtin_traits("array").contains(&"Display"));
        assert!(builtin_traits("map").contains(&"Eq"));
        assert!(builtin_traits("nope").is_empty());
    }
}
