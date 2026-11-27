//! Lexical tokens for Super.

use super_core::{Span, SymId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    // Punctuation
    Plus,                  // +
    Minus,                 // -
    Star,                  // *
    Slash,                 // /
    Percent,               // %
    Eq,                    // =
    EqEq,                  // ==
    NotEq,                 // !=
    Lt,                    // <
    LtEq,                  // <=
    Gt,                    // >
    GtEq,                  // >=
    PlusEq,                // +=
    MinusEq,               // -=
    StarEq,                // *=
    SlashEq,               // /=
    Dot,                   // .
    DotDot,                // ..
    Comma,                 // ,
    Semicolon,             // ;
    Colon,                 // :
    Question,              // ?
    Bang,                  // !
    At,                    // @
    Hash,                  // #
    Dollar,                // $
    QuoteStart,            // '{
    SpliceStart,           // ${
    FatArrow,              // =>
    Pipe,                  // |
    Amp,                   // &
    Caret,                 // ^
    Tilde,                 // ~
    AmpAmp,                // &&
    PipePipe,              // ||
    LessLess,              // <<
    GreaterGreater,        // >>
    GreaterGreaterGreater, // >>>

    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]

    // Keywords
    When,
    /// Channel / concurrency multiplex (`select { recv => … after … => … }`).
    /// Replaces subject-less `when { … }` (A3); `when` without a subject remains accepted.
    Select,
    Ensure,
    Else,
    Each,
    For,
    /// `par 0..N { … }` — parallel loop (replaces `for parallel`).
    Par,
    While,
    Loop,
    Break,
    Continue,
    In,
    Return,
    Fail,
    /// Unrecoverable abort (`panic` / `panic "msg"`). Distinct from `fail` (`T!`).
    Panic,
    None,
    Or,
    Err,
    Test,
    If,
    State,
    Await,
    Use,
    Embed,
    True,
    False,
    Scope,
    Spawn,
    Parallel,
    Chan,
    Shared,
    After,
    Rewrite,
    Type,
    Code,
    /// `+Name = trait { … }` — a method contract.
    Trait,

    // Literals / identifiers
    Ident,
    Int,
    Float,
    String,

    /// `//# …` executable meta-comment (content in `sym`, excluding `//#` prefix).
    MetaCommentLine,
    /// `/*# … #*/` executable meta-comment block.
    MetaCommentBlock,
    /// Outer documentation: `///` line or `/** … */` block. Body text is in `sym`.
    DocOuter,
    /// Inner documentation: `//!` line or `/*! … */` block. Body text is in `sym`.
    DocInner,

    Eof,
    Error,
}

impl TokenKind {
    pub fn from_ident(name: &str) -> Self {
        match name {
            "when" => Self::When,
            "select" => Self::Select,
            "ensure" => Self::Ensure,
            "else" => Self::Else,
            "each" => Self::Each,
            "for" => Self::For,
            "par" => Self::Par,
            "while" => Self::While,
            "loop" => Self::Loop,
            "break" => Self::Break,
            "continue" => Self::Continue,
            "in" => Self::In,
            "return" => Self::Return,
            "fail" => Self::Fail,
            "panic" => Self::Panic,
            "none" => Self::None,
            "or" => Self::Or,
            "err" => Self::Err,
            "test" => Self::Test,
            "if" => Self::If,
            "state" => Self::State,
            "await" => Self::Await,
            "use" => Self::Use,
            "embed" => Self::Embed,
            "true" => Self::True,
            "false" => Self::False,
            "scope" => Self::Scope,
            "spawn" => Self::Spawn,
            "parallel" => Self::Parallel,
            "chan" => Self::Chan,
            "shared" => Self::Shared,
            "after" => Self::After,
            "rewrite" => Self::Rewrite,
            "type" => Self::Type,
            "Code" => Self::Code,
            "trait" => Self::Trait,
            _ => Self::Ident,
        }
    }
}

/// Every reserved keyword, in the order the specification lists them.
pub const RESERVED_KEYWORDS: &[&str] = &[
    "after", "await", "break", "chan", "Code", "continue", "each", "else", "embed", "ensure",
    "err", "fail", "false", "for", "if", "in", "loop", "none", "or", "panic", "par", "parallel",
    "return", "rewrite", "scope", "select", "shared", "spawn", "state", "test", "trait", "true",
    "type", "use", "when", "while",
];

/// Keywords that only have meaning in one grammatical position and are ordinary
/// identifiers everywhere else. The parser asks this table instead of comparing
/// identifier text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextualKeyword {
    /// `use m.{a as b}` / `use m as n` — import rename.
    As,
    /// Implicit element in `for`, `when`, and pipeline closures.
    It,
    /// Implicit position in `for` and pipeline closures.
    Index,
    /// Receiver parameter of a method (`self` / `@self`).
    SelfValue,
    /// `_` — wildcard pattern and inferred type.
    Underscore,
}

impl ContextualKeyword {
    pub const ALL: [ContextualKeyword; 5] = [
        ContextualKeyword::As,
        ContextualKeyword::It,
        ContextualKeyword::Index,
        ContextualKeyword::SelfValue,
        ContextualKeyword::Underscore,
    ];

    pub fn from_text(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kw| kw.as_str() == text)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            ContextualKeyword::As => "as",
            ContextualKeyword::It => "it",
            ContextualKeyword::Index => "index",
            ContextualKeyword::SelfValue => "self",
            ContextualKeyword::Underscore => "_",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub sym: Option<SymId>,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self {
            kind,
            span,
            sym: None,
        }
    }

    pub fn with_sym(kind: TokenKind, span: Span, sym: SymId) -> Self {
        Self {
            kind,
            span,
            sym: Some(sym),
        }
    }
}
