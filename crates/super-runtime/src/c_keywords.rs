//! C23 keywords that cannot be emitted as identifiers.
//!
//! [`C_KEYWORDS`] is the keyword set of ISO C23 §6.4.1: the primary spellings,
//! the obsolescent underscore spellings that are still keywords, and the
//! interchange-float keywords `cc -std=c23` reserves. A Super name that
//! sanitizes to one of these is emitted with a trailing `_` (`short` becomes
//! `short_`). String keys stay untouched, so field lookup still uses the
//! source name.

/// Every keyword reserved by ISO C23 §6.4.1, sorted for binary search.
pub const C_KEYWORDS: &[&str] = &[
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_BitInt",
    "_Bool",
    "_Complex",
    "_Decimal128",
    "_Decimal32",
    "_Decimal64",
    "_Float128",
    "_Float128x",
    "_Float16",
    "_Float32",
    "_Float32x",
    "_Float64",
    "_Float64x",
    "_Generic",
    "_Imaginary",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "alignas",
    "alignof",
    "auto",
    "bool",
    "break",
    "case",
    "char",
    "const",
    "constexpr",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "false",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "nullptr",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "struct",
    "switch",
    "thread_local",
    "true",
    "typedef",
    "typeof",
    "typeof_unqual",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
];

pub fn is_c_keyword(name: &str) -> bool {
    C_KEYWORDS.binary_search(&name).is_ok()
}

/// Turn a Super name into a C identifier.
///
/// Non-identifier characters become `_`. A leading digit is prefixed with `_`.
/// A result that is a [`C_KEYWORDS`] entry gains a trailing `_`.
pub fn c_ident(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for (i, ch) in source.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
        if i == 0 && out.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            out.insert(0, '_');
        }
    }
    if out.is_empty() {
        return "_anon".to_string();
    }
    if is_c_keyword(&out) {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyword_list_is_sorted_and_unique() {
        assert!(C_KEYWORDS.is_sorted());
        let mut unique = C_KEYWORDS.to_vec();
        unique.dedup();
        assert_eq!(unique.len(), C_KEYWORDS.len());
        assert_eq!(C_KEYWORDS.len(), 66);
    }

    #[test]
    fn every_c_keyword_is_renamed_and_ordinary_names_are_not() {
        for word in C_KEYWORDS {
            let escaped = c_ident(word);
            assert_eq!(escaped, format!("{word}_"), "{word}");
            assert!(!is_c_keyword(&escaped), "{escaped} is still a keyword");
        }
        assert_eq!(c_ident("balance"), "balance");
        assert_eq!(c_ident("short_"), "short_");
        assert_eq!(c_ident("content-type"), "content_type");
        assert_eq!(c_ident("Short"), "Short");
        assert_eq!(c_ident(""), "_anon");
        assert_eq!(c_ident("1int"), "_1int");
    }
}
