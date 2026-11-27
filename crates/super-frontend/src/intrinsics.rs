//! Compiler-provided `#` macros.
//!
//! The `#` namespace holds two kinds of macros: pattern macros declared in Super
//! source (`+#name { (pattern) => { template } }`, see [`crate::macros`]) and the
//! intrinsics listed here, which the compiler implements because they need
//! information no source macro can see (build configuration, layout, source
//! text of the call site). Every intrinsic is resolved by name inside the `#`
//! namespace only; none of them is reserved as a bare identifier.

/// Item attributes: `#name` / `#name(args)` written on the line before an item
/// or loop.
pub const ATTRIBUTE_MACROS: &[&str] = &[
    "assert_size",
    "blocking",
    "cfg",
    "delete",
    "get",
    "inline",
    "inspect",
    "law",
    "post",
    "proof",
    "put",
    "route",
    "simd",
    "symmetric",
    "system",
    "template_html",
    "template_text",
    "unroll",
];

pub fn is_attribute_macro(name: &str) -> bool {
    ATTRIBUTE_MACROS.binary_search(&name).is_ok()
}

/// Intrinsics evaluated while a macro call is expanded. They operate on tokens
/// and may appear in templates or directly in source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenIntrinsic {
    /// `#stringify(tokens)` — string literal holding the tokens' source text.
    Stringify,
    /// `#concat(lit, …)` — string literal joining literal tokens.
    Concat,
    /// `#location` — `"line:column"` of the outermost call being expanded.
    Location,
    /// `#upper(token)` / `#lower(token)` — the identifier or string, case-mapped.
    Upper,
    Lower,
    /// `#error([anchor,] message)` — report a compile error; expands to nothing.
    Error,
    /// `#ensure_distinct(message, (key…) (key…) …)` — compile error on the first
    /// repeated key group; `{}` in the message is replaced by the key text.
    EnsureDistinct,
}

impl TokenIntrinsic {
    pub const ALL: [TokenIntrinsic; 7] = [
        TokenIntrinsic::Stringify,
        TokenIntrinsic::Concat,
        TokenIntrinsic::Location,
        TokenIntrinsic::Upper,
        TokenIntrinsic::Lower,
        TokenIntrinsic::Error,
        TokenIntrinsic::EnsureDistinct,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|intrinsic| intrinsic.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            TokenIntrinsic::Stringify => "stringify",
            TokenIntrinsic::Concat => "concat",
            TokenIntrinsic::Location => "location",
            TokenIntrinsic::Upper => "upper",
            TokenIntrinsic::Lower => "lower",
            TokenIntrinsic::Error => "error",
            TokenIntrinsic::EnsureDistinct => "ensure_distinct",
        }
    }
}

/// `#goal_type` — the expected type at the use site (metaprogramming reflection).
pub const GOAL_TYPE: &str = "goal_type";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_table_is_sorted_for_binary_search() {
        let mut sorted = ATTRIBUTE_MACROS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, ATTRIBUTE_MACROS);
        assert!(is_attribute_macro("cfg"));
        assert!(is_attribute_macro("get"));
        assert!(!is_attribute_macro("routes"));
        assert!(!is_attribute_macro("assert"));
    }

    #[test]
    fn token_intrinsics_round_trip_names() {
        for intrinsic in TokenIntrinsic::ALL {
            assert_eq!(TokenIntrinsic::from_name(intrinsic.name()), Some(intrinsic));
        }
        assert_eq!(TokenIntrinsic::from_name("routes"), None);
    }
}
