//! A parser for Valve's text `KeyValues` format (`.vdf`, `.acf`).
//!
//! Grammar handled (everything Steam writes to `libraryfolders.vdf`,
//! `appmanifest_*.acf`, `loginusers.vdf` and `config.vdf`):
//!
//! ```text
//! document := pair*
//! pair     := key ( value | '{' pair* '}' ) [conditional]
//! key      := quoted | bare
//! value    := quoted | bare
//! quoted   := '"' ( escape | any char except '"' and '\' )* '"'
//! escape   := '\' ( 'n' | 't' | 'r' | '\' | '"' )   (any other `\x` keeps both chars)
//! bare     := run of chars that are not whitespace, '"', '{', '}'
//! conditional := '[' ... ']'   (e.g. `[$WIN32]`; parsed and ignored)
//! comment  := '//' to end of line
//! ```
//!
//! Keys may repeat; [`Object`] keeps every pair in file order. Lookups with
//! [`Object::get`] are ASCII-case-insensitive (Steam is inconsistent about
//! `AppState` vs `appstate`) and return the *first* match.
//!
//! A conditional is recognised only where the grammar allows one, right after
//! a complete pair, and must close on the line it opens. Everywhere else a
//! `[` is an ordinary character of a bare token.

use std::fmt;
use std::str::Chars;

/// A `KeyValues` value: a string or a nested object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A quoted or bare string, unescaped.
    Str(String),
    /// A `{ ... }` block.
    Object(Object),
}

impl Value {
    /// The string, if this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Object(_) => None,
        }
    }

    /// The object, if this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            Value::Str(_) => None,
        }
    }
}

/// An ordered list of key/value pairs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Object {
    /// Pairs in file order. Duplicate keys are kept.
    pub pairs: Vec<(String, Value)>,
}

impl Object {
    /// First value whose key equals `key`, ignoring ASCII case.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    /// First string value for `key`.
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// First object value for `key`.
    #[must_use]
    pub fn get_object(&self, key: &str) -> Option<&Object> {
        self.get(key).and_then(Value::as_object)
    }

    /// Iterate over pairs in file order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.pairs.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// A parse failure with a 1-based position.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct ParseError {
    /// 1-based line.
    pub line: usize,
    /// 1-based column, counted in chars.
    pub column: usize,
    /// What went wrong, e.g. `unterminated string`, `unexpected '}'`,
    /// `missing value for key "appid"`, `unclosed '{' opened at line 3`.
    pub message: String,
}

/// Parse a whole document. A leading UTF-8 BOM is skipped.
///
/// Error positions point at the offending token: the opening `"` of an
/// unterminated string, the key that has no value, the stray brace, or the
/// end of input for an unclosed `{`.
///
/// # Errors
/// [`ParseError`] on unterminated strings, unbalanced braces, a key without a
/// value, or a nesting depth over [`MAX_DEPTH`].
///
/// # Examples
/// ```
/// let manifest = uncork_steam::vdf::parse(
///     "\"AppState\"\n{\n\t\"appid\"\t\t\"287450\"\n}\n",
/// )?;
/// let state = manifest.get_object("appstate").expect("AppState block");
/// assert_eq!(state.get_str("AppID"), Some("287450"));
/// # Ok::<(), uncork_steam::vdf::ParseError>(())
/// ```
pub fn parse(text: &str) -> Result<Object, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    parse_block(&mut Lexer::new(text), None, 0)
}

/// Maximum `{` nesting accepted by [`parse`] (protects against stack exhaustion
/// on hostile input).
pub const MAX_DEPTH: usize = 64;

/// Render an object back to text in Steam's style (tabs, quoted keys and
/// values, `\\` and `\"` escaped). `parse(&to_string(o)) == o` for every `o`
/// nested no deeper than [`MAX_DEPTH`].
#[must_use]
pub fn to_string(object: &Object) -> String {
    let mut out = String::new();
    write_pairs(&mut out, object, 0);
    out
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_string(self))
    }
}

/// Parse pairs until the closing brace of the block opened at `opened_at`, or
/// until end of input for the document itself (`opened_at == None`).
fn parse_block(
    lexer: &mut Lexer<'_>,
    opened_at: Option<Pos>,
    depth: usize,
) -> Result<Object, ParseError> {
    let mut object = Object::default();
    loop {
        let token = lexer.next_token()?;
        let key = match (token.kind, opened_at) {
            (Kind::Str(key), _) => key,
            (Kind::Close, Some(_)) | (Kind::Eof, None) => return Ok(object),
            (Kind::Eof, Some(open)) => {
                let message = format!("unclosed '{{' opened at line {}", open.line);
                return Err(token.pos.error(message));
            }
            (Kind::Close, None) => return Err(token.pos.error("unexpected '}'")),
            (Kind::Open, _) => return Err(token.pos.error("unexpected '{'")),
        };
        let value = parse_value(lexer, &key, token.pos, depth)?;
        lexer.skip_conditional()?;
        object.pairs.push((key, value));
    }
}

/// Parse the value (string or block) that follows `key`.
fn parse_value(
    lexer: &mut Lexer<'_>,
    key: &str,
    key_pos: Pos,
    depth: usize,
) -> Result<Value, ParseError> {
    let token = lexer.next_token()?;
    match token.kind {
        Kind::Str(value) => Ok(Value::Str(value)),
        Kind::Open if depth >= MAX_DEPTH => {
            let message = format!("'{{' nested deeper than {MAX_DEPTH} levels");
            Err(token.pos.error(message))
        }
        Kind::Open => parse_block(lexer, Some(token.pos), depth + 1).map(Value::Object),
        Kind::Close | Kind::Eof => Err(key_pos.error(format!("missing value for key {key:?}"))),
    }
}

fn write_pairs(out: &mut String, object: &Object, depth: usize) {
    for (key, value) in &object.pairs {
        write_indent(out, depth);
        write_quoted(out, key);
        match value {
            Value::Str(s) => {
                out.push_str("\t\t");
                write_quoted(out, s);
                out.push('\n');
            }
            Value::Object(inner) => {
                out.push('\n');
                write_indent(out, depth);
                out.push_str("{\n");
                write_pairs(out, inner, depth + 1);
                write_indent(out, depth);
                out.push_str("}\n");
            }
        }
    }
}

fn write_indent(out: &mut String, depth: usize) {
    out.extend(std::iter::repeat_n('\t', depth));
}

/// Quote `s`, escaping only `\` and `"`. Raw tabs and newlines are written
/// as-is: the parser keeps them verbatim inside quotes, so this is lossless.
fn write_quoted(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

/// A 1-based position in the input, columns counted in chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    line: usize,
    column: usize,
}

impl Pos {
    fn error(self, message: impl Into<String>) -> ParseError {
        ParseError {
            line: self.line,
            column: self.column,
            message: message.into(),
        }
    }
}

#[derive(Debug)]
enum Kind {
    Str(String),
    Open,
    Close,
    Eof,
}

#[derive(Debug)]
struct Token {
    kind: Kind,
    pos: Pos,
}

struct Lexer<'a> {
    chars: Chars<'a>,
    pos: Pos,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            chars: text.chars(),
            pos: Pos { line: 1, column: 1 },
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.clone().next()
    }

    fn peek_second(&self) -> Option<char> {
        self.chars.clone().nth(1)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        if c == '\n' {
            self.pos.line += 1;
            self.pos.column = 1;
        } else {
            self.pos.column += 1;
        }
        Some(c)
    }

    /// Skip whitespace and `//` comments.
    fn skip_trivia(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else if c == '/' && self.peek_second() == Some('/') {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.bump();
                }
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_trivia();
        let pos = self.pos;
        let kind = match self.peek() {
            None => Kind::Eof,
            Some('{') => {
                self.bump();
                Kind::Open
            }
            Some('}') => {
                self.bump();
                Kind::Close
            }
            Some('"') => Kind::Str(self.quoted(pos)?),
            Some(_) => Kind::Str(self.bare()),
        };
        Ok(Token { kind, pos })
    }

    /// Read a quoted string; the lexer sits on its opening quote at `start`.
    fn quoted(&mut self, start: Pos) -> Result<String, ParseError> {
        self.bump();
        let mut s = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(s),
                Some('\\') => {
                    let Some(escaped) = self.bump() else { break };
                    if let Some(c) = unescape(escaped) {
                        s.push(c);
                    } else {
                        s.push('\\');
                        s.push(escaped);
                    }
                }
                Some(c) => s.push(c),
                None => break,
            }
        }
        Err(start.error("unterminated string"))
    }

    /// Read a bare token. The caller guarantees the next char starts one, so
    /// the result is never empty.
    fn bare(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '"' | '{' | '}') {
                break;
            }
            s.push(c);
            self.bump();
        }
        s
    }

    /// Consume a `[...]` conditional if one follows; it is ignored.
    fn skip_conditional(&mut self) -> Result<(), ParseError> {
        self.skip_trivia();
        if self.peek() != Some('[') {
            return Ok(());
        }
        let start = self.pos;
        while let Some(c) = self.bump() {
            match c {
                ']' => return Ok(()),
                '\n' => break,
                _ => {}
            }
        }
        Err(start.error("unterminated conditional"))
    }
}

fn unescape(c: char) -> Option<char> {
    match c {
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        '\\' | '"' => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn s(value: &str) -> Value {
        Value::Str(value.to_owned())
    }

    fn obj(pairs: Vec<(&str, Value)>) -> Object {
        Object {
            pairs: pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        }
    }

    fn err(text: &str) -> ParseError {
        parse(text).expect_err("input should be rejected")
    }

    fn at(line: usize, column: usize, message: &str) -> ParseError {
        ParseError {
            line,
            column,
            message: message.to_owned(),
        }
    }

    #[test]
    fn empty_and_trivia_only_documents_are_empty_objects() {
        for text in [
            "",
            "   \n\t\r\n",
            "// only a comment",
            "\u{feff}",
            "// a\n// b\n",
        ] {
            assert_eq!(parse(text), Ok(Object::default()), "input {text:?}");
        }
    }

    #[test]
    fn parses_steam_style_nested_document() {
        let text = "\"AppState\"\n{\n\t\"appid\"\t\t\"287450\"\n\t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"english\"\n\t}\n}\n";
        let expected = obj(vec![(
            "AppState",
            Value::Object(obj(vec![
                ("appid", s("287450")),
                (
                    "UserConfig",
                    Value::Object(obj(vec![("language", s("english"))])),
                ),
            ])),
        )]);
        assert_eq!(parse(text), Ok(expected));
    }

    #[test]
    fn leading_bom_is_skipped_and_not_counted_as_a_column() {
        assert_eq!(parse("\u{feff}\"a\" \"b\""), Ok(obj(vec![("a", s("b"))])));
        assert_eq!(
            err("\u{feff}\"a\""),
            at(1, 1, "missing value for key \"a\"")
        );
    }

    #[test]
    fn bom_after_the_start_is_ordinary_text() {
        let parsed = parse("a \u{feff}b").expect("valid");
        assert_eq!(parsed.get_str("a"), Some("\u{feff}b"));
    }

    #[test]
    fn bare_tokens_stop_at_whitespace_quotes_and_braces() {
        let parsed = parse("key value\nk2\"quoted\"\nblock{inner x}").expect("valid");
        assert_eq!(
            parsed,
            obj(vec![
                ("key", s("value")),
                ("k2", s("quoted")),
                ("block", Value::Object(obj(vec![("inner", s("x"))]))),
            ])
        );
    }

    #[test]
    fn bare_tokens_keep_backslashes_and_slashes_verbatim() {
        let parsed = parse(r"path C:\new\dir url http://x/y a /b").expect("valid");
        assert_eq!(parsed.get_str("path"), Some(r"C:\new\dir"));
        assert_eq!(parsed.get_str("url"), Some("http://x/y"));
        assert_eq!(parsed.get_str("a"), Some("/b"));
    }

    #[test]
    fn quoted_escapes_are_unescaped() {
        let parsed = parse(r#""k" "a\\b\"c\nd\te\rf""#).expect("valid");
        assert_eq!(parsed.get_str("k"), Some("a\\b\"c\nd\te\rf"));
    }

    #[test]
    fn unknown_escapes_keep_both_chars() {
        let parsed = parse(r#""k" "D:\Games\x\é""#).expect("valid");
        assert_eq!(parsed.get_str("k"), Some(r"D:\Games\x\é"));
    }

    #[test]
    fn escaped_windows_path_from_libraryfolders() {
        let parsed = parse(r#""path"		"C:\\Program Files (x86)\\Steam""#).expect("valid");
        assert_eq!(
            parsed.get_str("path"),
            Some(r"C:\Program Files (x86)\Steam")
        );
    }

    #[test]
    fn empty_quoted_strings_are_valid_keys_and_values() {
        assert_eq!(parse(r#""" """#), Ok(obj(vec![("", s(""))])));
    }

    #[test]
    fn quoted_strings_may_span_lines_and_positions_follow() {
        let text = "\"k\" \"line one\nline two\"\n  }";
        assert_eq!(err(text), at(3, 3, "unexpected '}'"));
        let parsed = parse("\"k\" \"a\nb\"").expect("valid");
        assert_eq!(parsed.get_str("k"), Some("a\nb"));
    }

    #[test]
    fn comments_are_ignored_everywhere_between_tokens() {
        let text = "// header\n\"a\" // after key\n\"b\" // after value\n\"c\" { // open\n// inside\n\"d\" \"e\" } // close";
        assert_eq!(
            parse(text),
            Ok(obj(vec![
                ("a", s("b")),
                ("c", Value::Object(obj(vec![("d", s("e"))]))),
            ]))
        );
    }

    #[test]
    fn comment_markers_inside_quotes_are_text() {
        let parsed = parse(r#""url" "https://example.com//x""#).expect("valid");
        assert_eq!(parsed.get_str("url"), Some("https://example.com//x"));
    }

    #[test]
    fn conditionals_after_values_and_blocks_are_ignored() {
        let text =
            "\"a\" \"1\" [$WIN32]\n\"b\" { \"c\" \"2\" [!$X360 && $OSX] } [$WINDOWS]\n\"d\" \"3\"";
        assert_eq!(
            parse(text),
            Ok(obj(vec![
                ("a", s("1")),
                ("b", Value::Object(obj(vec![("c", s("2"))]))),
                ("d", s("3")),
            ]))
        );
    }

    #[test]
    fn a_bracket_in_key_or_value_position_is_a_bare_token() {
        let parsed = parse("[key] [value]").expect("valid");
        assert_eq!(parsed, obj(vec![("[key]", s("[value]"))]));
    }

    #[test]
    fn unterminated_conditional_points_at_the_bracket() {
        assert_eq!(
            err("\"a\" \"b\" [$WIN32\n\"c\" \"d\""),
            at(1, 9, "unterminated conditional")
        );
        assert_eq!(
            err("\"a\" \"b\"\n  [$WIN32"),
            at(2, 3, "unterminated conditional")
        );
    }

    #[test]
    fn duplicate_keys_are_kept_in_order_and_get_returns_the_first() {
        let parsed = parse("\"k\" \"1\"\n\"K\" \"2\"\n\"k\" { }").expect("valid");
        assert_eq!(parsed.pairs.len(), 3);
        assert_eq!(parsed.get_str("k"), Some("1"));
        let keys: Vec<&str> = parsed.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["k", "K", "k"]);
    }

    #[test]
    fn get_ignores_ascii_case_only() {
        let parsed = parse("\"AppState\" { } \"ÄPFEL\" \"x\"").expect("valid");
        assert!(parsed.get("appstate").is_some());
        assert!(parsed.get("APPSTATE").is_some());
        assert!(parsed.get("äpfel").is_none());
        assert!(parsed.get("missing").is_none());
    }

    #[test]
    fn typed_getters_check_the_value_kind() {
        let parsed = parse("\"str\" \"v\" \"obj\" { }").expect("valid");
        assert_eq!(parsed.get_str("str"), Some("v"));
        assert_eq!(parsed.get_object("str"), None);
        assert_eq!(parsed.get_str("obj"), None);
        assert_eq!(parsed.get_object("obj"), Some(&Object::default()));
        assert_eq!(s("x").as_str(), Some("x"));
        assert_eq!(s("x").as_object(), None);
        assert_eq!(Value::Object(Object::default()).as_str(), None);
    }

    #[test]
    fn unterminated_string_points_at_the_opening_quote() {
        assert_eq!(err("\"a\" \"b"), at(1, 5, "unterminated string"));
        assert_eq!(err("\"a\"\n\t\"b\nc\nd"), at(2, 2, "unterminated string"));
    }

    #[test]
    fn trailing_backslash_leaves_the_string_unterminated() {
        assert_eq!(err("\"a\" \"b\\"), at(1, 5, "unterminated string"));
        assert_eq!(err("\"a\" \"b\\\""), at(1, 5, "unterminated string"));
    }

    #[test]
    fn stray_closing_brace_at_top_level() {
        assert_eq!(err("\"a\" \"b\"\n}"), at(2, 1, "unexpected '}'"));
        assert_eq!(err("}"), at(1, 1, "unexpected '}'"));
    }

    #[test]
    fn opening_brace_where_a_key_is_expected() {
        assert_eq!(err("{ }"), at(1, 1, "unexpected '{'"));
        assert_eq!(err("\"a\" \"b\" {"), at(1, 9, "unexpected '{'"));
        assert_eq!(err("\"a\" { { } }"), at(1, 7, "unexpected '{'"));
    }

    #[test]
    fn key_without_value_points_at_the_key() {
        assert_eq!(
            err("\"appid\""),
            at(1, 1, "missing value for key \"appid\"")
        );
        assert_eq!(
            err("\"AppState\"\n{\n\t\"appid\"\n}"),
            at(3, 2, "missing value for key \"appid\"")
        );
        assert_eq!(err("bare"), at(1, 1, "missing value for key \"bare\""));
    }

    #[test]
    fn missing_value_message_escapes_the_key() {
        assert_eq!(
            err("\"say \\\"hi\\\"\""),
            at(1, 1, r#"missing value for key "say \"hi\"""#)
        );
    }

    #[test]
    fn unclosed_brace_reports_its_line_at_end_of_input() {
        assert_eq!(
            err("\"a\"\n{\n\t\"b\"\n\t{\n\t\t\"c\" \"d\"\n\t}\n"),
            at(7, 1, "unclosed '{' opened at line 2")
        );
        assert_eq!(
            err("\"a\" {\n\"b\" {\n\"c\" \"d\""),
            at(3, 8, "unclosed '{' opened at line 2")
        );
    }

    #[test]
    fn columns_count_chars_not_bytes() {
        assert_eq!(err("\"ключ\" \"значение\" }"), at(1, 19, "unexpected '}'"));
        assert_eq!(err("🎮 🕹️ }"), at(1, 6, "unexpected '}'"));
    }

    #[test]
    fn tabs_count_as_one_column() {
        assert_eq!(err("\t\t}"), at(1, 3, "unexpected '}'"));
    }

    #[test]
    fn crlf_line_endings_are_handled() {
        let text = "\"AppState\"\r\n{\r\n\t\"appid\"\t\t\"287450\"\r\n}\r\n";
        let parsed = parse(text).expect("valid");
        assert_eq!(
            parsed
                .get_object("AppState")
                .and_then(|state| state.get_str("appid")),
            Some("287450")
        );
        assert_eq!(
            err("\"a\" \"b\"\r\n\r\n  }\r\n"),
            at(3, 3, "unexpected '}'")
        );
    }

    fn nested(depth: usize) -> String {
        format!("{}{}", "\"k\" {".repeat(depth), "}".repeat(depth))
    }

    #[test]
    fn nesting_up_to_max_depth_is_accepted() {
        let mut current = &parse(&nested(MAX_DEPTH)).expect("valid");
        for _ in 0..MAX_DEPTH {
            current = current.get_object("k").expect("nested block");
        }
        assert!(current.pairs.is_empty());
    }

    #[test]
    fn nesting_beyond_max_depth_is_rejected_at_the_offending_brace() {
        let column = "\"k\" {".len() * MAX_DEPTH + "\"k\" ".len() + 1;
        assert_eq!(
            err(&nested(MAX_DEPTH + 1)),
            at(1, column, "'{' nested deeper than 64 levels")
        );
    }

    #[test]
    fn hostile_deep_nesting_does_not_overflow_the_stack() {
        let text = "a {".repeat(100_000);
        assert_eq!(err(&text).message, "'{' nested deeper than 64 levels");
    }

    #[test]
    fn to_string_uses_steam_layout() {
        let object = obj(vec![(
            "libraryfolders",
            Value::Object(obj(vec![(
                "0",
                Value::Object(obj(vec![
                    ("path", s(r"C:\Program Files (x86)\Steam")),
                    ("label", s("")),
                    ("apps", Value::Object(obj(vec![("287450", s("1"))]))),
                ])),
            )])),
        )]);
        let expected = concat!(
            "\"libraryfolders\"\n",
            "{\n",
            "\t\"0\"\n",
            "\t{\n",
            "\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n",
            "\t\t\"label\"\t\t\"\"\n",
            "\t\t\"apps\"\n",
            "\t\t{\n",
            "\t\t\t\"287450\"\t\t\"1\"\n",
            "\t\t}\n",
            "\t}\n",
            "}\n",
        );
        assert_eq!(to_string(&object), expected);
        assert_eq!(object.to_string(), expected);
    }

    #[test]
    fn to_string_of_empty_object_is_empty() {
        assert_eq!(to_string(&Object::default()), "");
    }

    #[test]
    fn to_string_escapes_only_quotes_and_backslashes() {
        let object = obj(vec![("k\"ey", s("a\\b\tc\nd{}"))]);
        assert_eq!(to_string(&object), "\"k\\\"ey\"\t\t\"a\\\\b\tc\nd{}\"\n");
    }

    fn tricky_char() -> impl Strategy<Value = char> {
        prop_oneof![
            3 => prop::sample::select(vec![
                '"', '\\', '{', '}', '[', ']', '/', ' ', '\t', '\n', '\r',
                '\u{feff}', 'n', 't', 'r', 'é', '日', '🎮',
            ]),
            1 => any::<char>(),
        ]
    }

    fn tricky_string() -> impl Strategy<Value = String> {
        prop::collection::vec(tricky_char(), 0..10).prop_map(|chars| chars.into_iter().collect())
    }

    fn arb_object() -> impl Strategy<Value = Object> {
        let pairs = |value: BoxedStrategy<Value>| {
            prop::collection::vec((tricky_string(), value), 0..5).prop_map(|pairs| Object { pairs })
        };
        let leaf = pairs(tricky_string().prop_map(Value::Str).boxed());
        leaf.prop_recursive(5, 48, 5, move |inner| {
            pairs(
                prop_oneof![
                    tricky_string().prop_map(Value::Str),
                    inner.prop_map(Value::Object),
                ]
                .boxed(),
            )
        })
    }

    proptest! {
        #[test]
        fn round_trips_through_text(object in arb_object()) {
            prop_assert_eq!(parse(&to_string(&object)), Ok(object));
        }

        #[test]
        fn never_panics_on_arbitrary_input(text in tricky_string()) {
            let _ = parse(&text);
        }

        #[test]
        fn never_panics_on_arbitrary_unicode(text in any::<String>()) {
            let _ = parse(&text);
        }

        #[test]
        fn error_positions_lie_within_the_input(text in tricky_string()) {
            if let Err(error) = parse(&text) {
                let body = text.strip_prefix('\u{feff}').unwrap_or(&text);
                let lines: Vec<&str> = body.split('\n').collect();
                prop_assert!(error.line >= 1 && error.line <= lines.len());
                let line_chars = lines[error.line - 1].chars().count();
                prop_assert!(error.column >= 1 && error.column <= line_chars + 1);
            }
        }
    }
}
