/// <reference types="tree-sitter-cli/dsl" />
// @ts-check
// Practical highlighting grammar for Super (ambiguity-tolerant).
// Exposes enough structure for Zed/Neovim/Helix: types, metaprogramming, scopes.
// After editing: `cd editors/tree-sitter-super && npm run build` (needs tree-sitter-cli).

module.exports = grammar({
  name: 'super',

  // Whitespace only. Comments must NOT be extras: a multi-part `string` rule
  // would otherwise let `/\/\*…\*\//` fire between `"` and content, so a
  // literal `"/*"` eats the file until the next `*/` (e.g. inside `"*/*"`).
  extras: $ => [/\s/],

  word: $ => $.ident,

  conflicts: $ => [
    [$.quote_expr, $._token],
    [$.splice_expr, $._token],
  ],

  rules: {
    source_file: $ => repeat($._token),

    // `///` documents the next declaration. A leading `////` is an ordinary comment
    // (higher precedence, so it wins the whole line).
    doc_comment: _ => token(prec(5, /\/\/\/[^\n]*/)),
    // `//!` documents the enclosing module or type.
    inner_doc_comment: _ => token(prec(5, /\/\/![^\n]*/)),
    // `/** … */` outer block doc. The first body character must not be `*` or `/`,
    // so `/***` and `/**/` stay ordinary block comments.
    doc_block: _ => token(prec(5, /\/\*\*[^/*][\s\S]*?\*\//)),
    inner_doc_block: _ => token(prec(6, /\/\*![\s\S]*?\*\//)),

    comment: _ => choice(
      token(prec(7, /\/\/\/\/[^\n]*/)),
      token(prec(1, /\/\/[^\n]*/)),
      token(prec(1, /\/\*[\s\S]*?\*\//)),
    ),

    meta_comment: _ => token(prec(6, choice(
      seq('//#', /.*/),
      seq('/*#', /[\s\S]*?/, '#*/'),
    ))),

    _token: $ => choice(
      $.meta_comment,
      $.doc_comment,
      $.inner_doc_comment,
      $.doc_block,
      $.inner_doc_block,
      $.comment,
      $.quote_expr,
      $.splice_expr,
      $.comptime_binding,
      $.raw_string,
      $.string,
      $.number,
      $.keyword,
      $.builtin_type,
      $.core_trait,
      $.export_marker,
      $.private_marker,
      $.mut_marker,
      $.goal_type,
      $.operator,
      $.punctuation,
      ':',
      '{', '}',
      '[', ']',
      '(', ')',
      $.ident,
    ),

    quote_expr: $ => prec(5, seq("'{", repeat($._token), '}')),
    splice_expr: $ => prec(5, seq('${', repeat($._token), '}')),

    comptime_binding: _ => token(prec(2, /#[A-Za-z_][A-Za-z0-9_]*/)),

    goal_type: _ => token(prec(3, '#goal_type')),

    keyword: _ => token(choice(
      'use', 'as', 'mod', 'embed', 'test', 'state', 'when', 'ensure', 'else', 'for', 'each', 'while', 'loop',
      'break', 'continue', 'in',
      'return', 'fail', 'panic', 'or', 'err', 'if', 'await',
      'scope', 'spawn', 'parallel', 'par', 'chan', 'shared', 'after',
      'get', 'post', 'put', 'delete',
      'rewrite', 'trait', 'type', 'Code',
      'true', 'false', 'none',
      'it', 'index',
    )),

    builtin_type: _ => token(prec(2, choice(
      'i8', 'i16', 'i32', 'i64',
      'u8', 'u16', 'u32', 'u64',
      'f32', 'f64',
      'bool', 'str', 'void',
    ))),

    core_trait: _ => token(prec(2, choice(
      'Display', 'Debug', 'ToString', 'Eq', 'Ord', 'Hash', 'Clone', 'Copy',
      'Default', 'From', 'Into', 'AsRef', 'Borrow', 'Iter',
      'Add', 'Sub', 'Mul', 'Div', 'Rem', 'Neg', 'Not', 'Index', 'Error', 'Drop',
    ))),

    export_marker: _ => token(prec(2, /\+[A-Za-z_][A-Za-z0-9_]*/)),
    private_marker: _ => token(prec(2, /\-[A-Za-z_][A-Za-z0-9_]*/)),
    mut_marker: _ => token(prec(2, /@[A-Za-z_][A-Za-z0-9_]*/)),

    operator: _ => token(choice(
      '=>', '..=', '..', '==', '!=', '<=', '>=',
      '+=', '-=', '*=', '/=',
      '&&', '||',
      '<<', '>>', '>>>',
      '&', '|', '^', '~',
      '+', '-', '*', '/', '%',
      '<', '>', '=', '?', '!',
      '#', '$',
    )),

    punctuation: _ => token(choice(',', '|', '.')),

    ident: _ => /[A-Za-z_][A-Za-z0-9_]*/,

    // Decimal / hex with optional `_` separators (0x1_0000_0000, 1_024, 3.14).
    // Float form requires a digit after `.` so `0..len(xs)` stays Int / `..` / call.
    number: _ => token(choice(
      /0[xX][0-9a-fA-F](_?[0-9a-fA-F])*/,
      /\d(_?\d)*(\.\d(_?\d)*)?([eE][+-]?\d(_?\d)*)?/,
    )),

    // `r"…"` and `r#"…"#` (no escapes); must precede normal strings in `_token`.
    // Approximate raw literals for highlighting (`r"…"`, `r#"…"#`, …).
    raw_string: _ => token(prec(3, /r#*"[^"\n]*"#*/)),

    string: $ => seq(
      '"',
      repeat(choice(
        $.string_content,
        $.escape_sequence,
        $.invalid_escape,
        $.interpolation,
      )),
      '"',
    ),

    string_content: _ => token.immediate(/[^"{\\]+/),
    escape_sequence: _ => token.immediate(/\\(?:[nrt0\\"'{}]|x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\})/),
    invalid_escape: _ => token.immediate(/\\./),
    interpolation: $ => seq(token.immediate('{'), /[^}]*/, '}'),
  },
});
