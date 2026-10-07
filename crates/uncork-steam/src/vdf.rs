//! A parser for Valve's text KeyValues format (`.vdf`, `.acf`).
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

use std::fmt;

/// A KeyValues value: a string or a nested object.
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
        let _ = key;
        todo!()
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
/// # Errors
/// [`ParseError`] on unterminated strings, unbalanced braces, a key without a
/// value, or a nesting depth over [`MAX_DEPTH`].
pub fn parse(text: &str) -> Result<Object, ParseError> {
    let _ = text;
    todo!()
}

/// Maximum `{` nesting accepted by [`parse`] (protects against stack exhaustion
/// on hostile input).
pub const MAX_DEPTH: usize = 64;

/// Render an object back to text in Steam's style (tabs, quoted keys and
/// values, `\\` and `\"` escaped). `parse(&to_string(o)) == o` for every `o`.
#[must_use]
pub fn to_string(object: &Object) -> String {
    let _ = object;
    todo!()
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_string(self))
    }
}
