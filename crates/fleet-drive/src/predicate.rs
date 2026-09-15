//! Predicate syntax shared by the scenario runner and application.
//!
//! The runner parses a predicate before the application ever sees it, so a malformed predicate
//! fails the scenario with a precise message instead of hanging an `await`. Both sides then
//! evaluate the *same* [`Predicate`] against the serialised `UiSnapshot`, which is why [`eval`]
//! takes a [`serde_json::Value`] rather than an application type.
//!
//! # Grammar
//!
//! ```text
//! predicate := atom ("&&" atom)*
//! atom      := "idle" | path "exists" | path "absent" | path op value
//! op        := "==" | "!=" | "~=" | ">" | "<"
//! path      := segment ("." segment)*
//! segment   := name ("[" digits "]")*
//! value     := "\"" text "\"" | word
//! ```
//!
//! There is no precedence, no disjunction and no parentheses: this is deliberately not a query
//! language. A scenario that needs computation needs a Rust test instead.
//!
//! # Values
//!
//! A value is a JSON scalar. `true`, `false`, `null` and anything that parses as a finite number
//! are read as such; every other unquoted token is a string, so `assert screen == Hub` works
//! without quoting. Inside double quotes only `\"` and `\\` are escapes and every other backslash
//! is literal, so a regular expression such as `terminal.text ~= "\d+"` survives unaltered.
//!
//! # Evaluation
//!
//! A path that does not resolve is *not* an error: every comparison over it is false, `exists` is
//! false and `absent` is true. `absent` also holds for a path that resolves to `null`, because the
//! snapshot reports an unavailable optional surface as `null` rather than by omitting the key.
//!
//! Comparisons are between JSON scalars. Two numbers compare numerically across integer and float;
//! otherwise values compare by JSON equality, falling back to the textual rendering of both sides
//! so an unquoted `3` still matches a label of `"3"`. `>` and `<` are numeric and are false for a
//! value that is not a number. `~=` matches a [`regex`] pattern against the textual rendering, and
//! is false for an array, an object or `null`.
//!
//! [`Evaluation`] reports the value actually found at every clause's path, because a caller
//! writing a failure message needs "`overlay` was `null`", not "false".

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A conjunction of atomic snapshot predicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    pub clauses: Vec<Clause>,
}

/// One atomic predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Clause {
    Idle,
    Compare {
        path: String,
        op: Operator,
        value: serde_json::Value,
    },
    Exists {
        path: String,
    },
    Absent {
        path: String,
    },
}

/// Supported comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Eq,
    NotEq,
    Regex,
    Greater,
    Less,
}

/// The snapshot field [`Clause::Idle`] reads.
const IDLE_PATH: &str = "idle.idle";

/// The snapshot field an `idle` failure reports, so a timeout names the busy counter.
const IDLE_REPORT_PATH: &str = "idle";

impl Operator {
    /// The operator as it is written in a scenario file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::NotEq => "!=",
            Self::Regex => "~=",
            Self::Greater => ">",
            Self::Less => "<",
        }
    }
}

impl fmt::Display for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Clause {
    /// The snapshot path this clause reads, which is every clause including `idle`.
    pub fn path(&self) -> &str {
        match self {
            Self::Idle => IDLE_REPORT_PATH,
            Self::Compare { path, .. } | Self::Exists { path } | Self::Absent { path } => path,
        }
    }
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::Exists { path } => write!(f, "{path} exists"),
            Self::Absent { path } => write!(f, "{path} absent"),
            Self::Compare { path, op, value } => write!(f, "{path} {op} {}", Scalar(value)),
        }
    }
}

impl fmt::Display for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, clause) in self.clauses.iter().enumerate() {
            if index > 0 {
                f.write_str(" && ")?;
            }
            write!(f, "{clause}")?;
        }
        Ok(())
    }
}

/// A JSON value rendered the way a predicate writes it: strings quoted, scalars bare.
struct Scalar<'a>(&'a Value);

impl fmt::Display for Scalar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::String(text) => write!(f, "{text:?}"),
            other => write!(f, "{other}"),
        }
    }
}

/// What one clause did against one snapshot, including the value that made it fail.
#[derive(Debug, Clone, PartialEq)]
pub struct ClauseOutcome {
    /// The clause this outcome belongs to.
    pub clause: Clause,
    /// Whether the clause held.
    pub satisfied: bool,
    /// The value at [`Clause::path`], or `None` when the path does not resolve.
    pub actual: Option<Value>,
}

impl ClauseOutcome {
    /// A one-line account of the clause and the value it saw.
    pub fn describe(&self) -> String {
        let verdict = if self.satisfied { "held" } else { "failed" };
        match &self.actual {
            Some(value) => format!(
                "`{}` {verdict}; {} is {}",
                self.clause,
                self.clause.path(),
                Scalar(value)
            ),
            None => format!(
                "`{}` {verdict}; {} is missing from the snapshot",
                self.clause,
                self.clause.path()
            ),
        }
    }
}

/// The result of evaluating a whole [`Predicate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// Whether every clause held.
    pub satisfied: bool,
    /// One outcome per clause, in source order; every clause is evaluated.
    pub clauses: Vec<ClauseOutcome>,
}

impl Evaluation {
    /// The clauses that did not hold.
    pub fn failures(&self) -> impl Iterator<Item = &ClauseOutcome> {
        self.clauses.iter().filter(|clause| !clause.satisfied)
    }

    /// A reason the predicate did not hold, or `None` when it did.
    pub fn failure_summary(&self) -> Option<String> {
        if self.satisfied {
            return None;
        }
        let reasons: Vec<_> = self.failures().map(ClauseOutcome::describe).collect();
        Some(reasons.join("; "))
    }
}

/// Parses the frozen predicate grammar.
///
/// # Errors
///
/// Returns a message naming the offending token and its 1-based column for any syntax error,
/// including an unparsable path and an invalid `~=` pattern.
pub fn parse(src: &str) -> anyhow::Result<Predicate> {
    let tokens = lex(src)?;
    let mut clauses = Vec::new();
    let mut cursor = 0;
    loop {
        clauses.push(parse_clause(src, &tokens, &mut cursor)?);
        match tokens.get(cursor) {
            None => break,
            Some(token) if token.kind == TokenKind::And => cursor += 1,
            Some(token) => {
                return Err(unexpected(src, token, "`&&` or the end of the predicate"));
            }
        }
    }
    Ok(Predicate { clauses })
}

/// Evaluates a predicate against a serialized snapshot.
///
/// # Errors
///
/// Returns an error only for a [`Predicate`] that [`parse`] would have rejected — one carrying an
/// unparsable path or an invalid regular expression — which a deserialized predicate can.
pub fn eval(predicate: &Predicate, snapshot: &serde_json::Value) -> anyhow::Result<Evaluation> {
    let mut clauses = Vec::with_capacity(predicate.clauses.len());
    for clause in &predicate.clauses {
        clauses.push(eval_clause(clause, snapshot)?);
    }
    Ok(Evaluation {
        satisfied: clauses.iter().all(|clause| clause.satisfied),
        clauses,
    })
}

fn eval_clause(clause: &Clause, snapshot: &Value) -> anyhow::Result<ClauseOutcome> {
    let actual = resolve_str(clause.path(), snapshot)?.cloned();
    let satisfied = match clause {
        Clause::Idle => resolve_str(IDLE_PATH, snapshot)? == Some(&Value::Bool(true)),
        Clause::Exists { .. } => actual.as_ref().is_some_and(|value| !value.is_null()),
        Clause::Absent { .. } => actual.as_ref().is_none_or(Value::is_null),
        Clause::Compare { op, value, .. } => match actual.as_ref() {
            None => false,
            Some(found) => match op {
                Operator::Eq => scalar_eq(found, value),
                Operator::NotEq => !scalar_eq(found, value),
                Operator::Greater => numeric_pair(found, value).is_some_and(|(a, b)| a > b),
                Operator::Less => numeric_pair(found, value).is_some_and(|(a, b)| a < b),
                Operator::Regex => {
                    let regex = compile(&pattern_of(value))?;
                    scalar_text(found).is_some_and(|text| regex.is_match(&text))
                }
            },
        },
    };
    Ok(ClauseOutcome {
        clause: clause.clone(),
        satisfied,
        actual,
    })
}

/// The pattern text of a `~=` value, so an unquoted `~= 200` is the pattern `200`.
fn pattern_of(value: &Value) -> Cow<'_, str> {
    scalar_text(value).unwrap_or(Cow::Borrowed(""))
}

fn compile(pattern: &str) -> anyhow::Result<regex::Regex> {
    regex::Regex::new(pattern)
        .map_err(|error| anyhow::anyhow!("`{pattern}` is not a valid regular expression: {error}"))
}

/// Equality between JSON scalars, tolerant of a value the grammar could only write unquoted.
fn scalar_eq(actual: &Value, expected: &Value) -> bool {
    if let (Value::Number(actual), Value::Number(expected)) = (actual, expected) {
        return match (actual.as_f64(), expected.as_f64()) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => actual == expected,
        };
    }
    if actual == expected {
        return true;
    }
    match (scalar_text(actual), scalar_text(expected)) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}

fn numeric_pair(actual: &Value, expected: &Value) -> Option<(f64, f64)> {
    Some((as_number(actual)?, as_number(expected)?))
}

fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

/// The textual rendering of a scalar; `None` for `null`, arrays and objects, which have no text.
fn scalar_text(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(text) => Some(Cow::Borrowed(text)),
        Value::Number(number) => Some(Cow::Owned(number.to_string())),
        Value::Bool(flag) => Some(Cow::Borrowed(if *flag { "true" } else { "false" })),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------

/// One step of a dotted path: an object key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Key(String),
    Index(usize),
}

fn resolve_str<'a>(path: &str, snapshot: &'a Value) -> anyhow::Result<Option<&'a Value>> {
    let segments = parse_path(path).map_err(|reason| anyhow::anyhow!("`{path}`: {reason}"))?;
    Ok(resolve(&segments, snapshot))
}

fn resolve<'a>(segments: &[Segment], snapshot: &'a Value) -> Option<&'a Value> {
    let mut cursor = snapshot;
    for segment in segments {
        cursor = match segment {
            Segment::Key(key) => cursor.get(key)?,
            Segment::Index(index) => cursor.get(*index)?,
        };
    }
    Some(cursor)
}

/// Splits a path into segments, rejecting every shape the snapshot cannot contain.
///
/// A segment is a bare name, `[<index>]`, or `["<key>"]`. The quoted form is what reaches the
/// two maps whose keys are not bare names: `lists["board.cards"]` carries a dot and
/// `targets["worktrees.row[0]"]` carries both a dot and brackets, and
/// `docs/TESTING-HARNESS.md` §3 freezes both of those names. Without it those surfaces are
/// published under keys no predicate can address, which is worse than not publishing them.
/// Inside the quotes only `\"` and `\\` are escapes, matching the value grammar in §2.
fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    if path.is_empty() {
        return Err("a path cannot be empty".to_owned());
    }
    let characters: Vec<char> = path.chars().collect();
    let mut segments = Vec::new();
    let mut at = 0;
    let mut expects_name = true;
    while at < characters.len() {
        match characters[at] {
            '[' => {
                let (segment, next) = parse_bracket(&characters, at, path)?;
                segments.push(segment);
                at = next;
                expects_name = false;
            }
            '.' => {
                if expects_name || at + 1 == characters.len() {
                    return Err(format!("`` is an empty path segment in `{path}`"));
                }
                at += 1;
                expects_name = true;
            }
            _ => {
                let start = at;
                while at < characters.len() && is_name_char(characters[at]) {
                    at += 1;
                }
                if at == start {
                    return Err(format!(
                        "`{}` is not allowed in the path segment `{path}`",
                        characters[at]
                    ));
                }
                if !expects_name {
                    return Err(format!("`{path}` has trailing text after `]`"));
                }
                segments.push(Segment::Key(characters[start..at].iter().collect()));
                expects_name = false;
            }
        }
    }
    if expects_name {
        return Err(format!("`` is an empty path segment in `{path}`"));
    }
    Ok(segments)
}

/// Parses one `[…]` step, which is either an array index or a quoted object key.
///
/// Returns the segment and the offset just past the closing `]`.
fn parse_bracket(characters: &[char], at: usize, path: &str) -> Result<(Segment, usize), String> {
    let mut cursor = at + 1;
    if characters.get(cursor) == Some(&'"') {
        cursor += 1;
        let mut key = String::new();
        loop {
            match characters.get(cursor) {
                None => return Err(format!("`{path}` has an unterminated quoted key")),
                Some('\\') => {
                    match characters.get(cursor + 1) {
                        Some(escaped @ ('"' | '\\')) => key.push(*escaped),
                        _ => {
                            return Err(format!(
                                "`{path}` escapes something other than `\\\"` or `\\\\` in a quoted key"
                            ));
                        }
                    }
                    cursor += 2;
                }
                Some('"') => {
                    cursor += 1;
                    break;
                }
                Some(character) => {
                    key.push(*character);
                    cursor += 1;
                }
            }
        }
        if characters.get(cursor) != Some(&']') {
            return Err(format!("`{path}` has an unclosed `[`"));
        }
        if key.is_empty() {
            return Err(format!("`{path}` has an empty quoted key"));
        }
        return Ok((Segment::Key(key), cursor + 1));
    }
    let start = cursor;
    while cursor < characters.len() && characters[cursor] != ']' {
        cursor += 1;
    }
    if cursor == characters.len() {
        return Err(format!("`{path}` has an unclosed `[`"));
    }
    let index: String = characters[start..cursor].iter().collect();
    match index.parse::<usize>() {
        Ok(index) => Ok((Segment::Index(index), cursor + 1)),
        Err(_) => Err(format!("`{index}` is not an array index in `{path}`")),
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

// ---------------------------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    And,
    Op(Operator),
    /// A double-quoted string, already unescaped.
    Quoted(String),
    /// A bare token: a path, an operator word, or an unquoted value.
    Word,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    /// The token exactly as written, for error messages.
    raw: String,
    /// 1-based character column of the token's first character.
    column: usize,
}

impl Token {
    /// The token's text: a quoted token's contents, otherwise the token itself.
    fn text(&self) -> &str {
        match &self.kind {
            TokenKind::Quoted(text) => text,
            _ => &self.raw,
        }
    }
}

/// Characters that end a bare word, because each one starts a token of its own.
const WORD_BREAK: [char; 7] = ['=', '!', '~', '>', '<', '&', '"'];

fn lex(src: &str) -> anyhow::Result<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c.is_whitespace() {
            index += 1;
            continue;
        }
        let column = index + 1;
        match c {
            '&' => {
                if chars.get(index + 1) != Some(&'&') {
                    return Err(syntax_error(
                        src,
                        column,
                        "expected `&&`, found a single `&`",
                    ));
                }
                tokens.push(Token {
                    kind: TokenKind::And,
                    raw: "&&".to_owned(),
                    column,
                });
                index += 2;
            }
            '=' | '!' | '~' => {
                if chars.get(index + 1) != Some(&'=') {
                    return Err(syntax_error(
                        src,
                        column,
                        format!("expected `{c}=`, found `{c}`"),
                    ));
                }
                let op = match c {
                    '=' => Operator::Eq,
                    '!' => Operator::NotEq,
                    _ => Operator::Regex,
                };
                tokens.push(Token {
                    kind: TokenKind::Op(op),
                    raw: op.as_str().to_owned(),
                    column,
                });
                index += 2;
            }
            '>' | '<' => {
                let op = if c == '>' {
                    Operator::Greater
                } else {
                    Operator::Less
                };
                tokens.push(Token {
                    kind: TokenKind::Op(op),
                    raw: op.as_str().to_owned(),
                    column,
                });
                index += 1;
            }
            '"' => {
                let (token, next) = lex_quoted(src, &chars, index)?;
                tokens.push(token);
                index = next;
            }
            _ => {
                let start = index;
                while index < chars.len() {
                    // A `["…"]` step is part of the path, not a value, so the quote that opens
                    // it must not end the word. `targets["worktrees.row[0]"].frame` is one
                    // token; a bare `"` anywhere else still starts a quoted value.
                    if chars[index] == '['
                        && let Some(next) = bracket_span(&chars, index)
                    {
                        index = next;
                        continue;
                    }
                    if chars[index].is_whitespace() || WORD_BREAK.contains(&chars[index]) {
                        break;
                    }
                    index += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Word,
                    raw: chars[start..index].iter().collect(),
                    column,
                });
            }
        }
    }
    Ok(tokens)
}

/// The offset just past the `]` closing the bracket step that starts at `open`.
///
/// `None` when the step is unbalanced, which leaves the word scanner to stop where it would
/// have stopped anyway so that [`parse_path`] reports the malformed path rather than the lexer
/// reporting a stray quote.
fn bracket_span(chars: &[char], open: usize) -> Option<usize> {
    let mut index = open + 1;
    if chars.get(index) == Some(&'"') {
        index += 1;
        loop {
            match chars.get(index)? {
                '\\' => index += 2,
                '"' => {
                    index += 1;
                    break;
                }
                _ => index += 1,
            }
        }
    }
    while chars.get(index)? != &']' {
        index += 1;
    }
    Some(index + 1)
}

/// Reads a double-quoted string. Only `\"` and `\\` are escapes, so a regular expression's own
/// backslashes reach [`regex`] unaltered.
fn lex_quoted(src: &str, chars: &[char], start: usize) -> anyhow::Result<(Token, usize)> {
    let column = start + 1;
    let mut text = String::new();
    let mut index = start + 1;
    while index < chars.len() {
        match chars[index] {
            '"' => {
                return Ok((
                    Token {
                        kind: TokenKind::Quoted(text),
                        raw: chars[start..=index].iter().collect(),
                        column,
                    },
                    index + 1,
                ));
            }
            '\\' if matches!(chars.get(index + 1), Some('"' | '\\')) => {
                text.push(chars[index + 1]);
                index += 2;
            }
            other => {
                text.push(other);
                index += 1;
            }
        }
    }
    Err(syntax_error(src, column, "unterminated quoted value"))
}

// ---------------------------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------------------------

fn parse_clause(src: &str, tokens: &[Token], cursor: &mut usize) -> anyhow::Result<Clause> {
    let head = tokens
        .get(*cursor)
        .ok_or_else(|| missing(src, "a path or `idle`"))?;
    if head.kind != TokenKind::Word {
        return Err(unexpected(src, head, "a path or `idle`"));
    }
    *cursor += 1;
    let is_end = matches!(
        tokens.get(*cursor).map(|token| &token.kind),
        None | Some(TokenKind::And)
    );
    if head.raw == "idle" && is_end {
        return Ok(Clause::Idle);
    }
    let path = head.raw.clone();
    if let Err(reason) = parse_path(&path) {
        return Err(syntax_error(src, head.column, reason));
    }

    let operator = tokens.get(*cursor).ok_or_else(|| {
        missing(
            src,
            format!("an operator after the path `{path}` (`==`, `!=`, `~=`, `>`, `<`, `exists` or `absent`)"),
        )
    })?;
    *cursor += 1;
    let op = match &operator.kind {
        TokenKind::Word if operator.raw == "exists" => return Ok(Clause::Exists { path }),
        TokenKind::Word if operator.raw == "absent" => return Ok(Clause::Absent { path }),
        TokenKind::Op(op) => *op,
        _ => {
            return Err(unexpected(
                src,
                operator,
                "an operator (`==`, `!=`, `~=`, `>`, `<`, `exists` or `absent`)",
            ));
        }
    };

    let literal = tokens
        .get(*cursor)
        .ok_or_else(|| missing(src, format!("a value after `{op}`")))?;
    *cursor += 1;
    let value = match &literal.kind {
        TokenKind::Quoted(text) => Value::String(text.clone()),
        TokenKind::Word => scalar_from_word(&literal.raw),
        TokenKind::Op(_) | TokenKind::And => {
            return Err(unexpected(src, literal, format!("a value after `{op}`")));
        }
    };
    if op == Operator::Regex
        && let Err(error) = compile(literal.text())
    {
        return Err(syntax_error(src, literal.column, error));
    }
    Ok(Clause::Compare { path, op, value })
}

/// Reads an unquoted value as the JSON scalar it looks like, and as a string otherwise.
fn scalar_from_word(word: &str) -> Value {
    match word {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        "null" => return Value::Null,
        _ => {}
    }
    let numeric = word.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+');
    if numeric {
        if let Ok(integer) = word.parse::<i64>() {
            return Value::from(integer);
        }
        if let Ok(float) = word.parse::<f64>()
            && let Some(number) = serde_json::Number::from_f64(float)
        {
            return Value::Number(number);
        }
    }
    Value::String(word.to_owned())
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

fn syntax_error(src: &str, column: usize, message: impl fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("invalid predicate `{src}`: {message}, at column {column}")
}

fn unexpected(src: &str, token: &Token, expected: impl fmt::Display) -> anyhow::Error {
    syntax_error(
        src,
        token.column,
        format!("expected {expected}, found `{}`", token.raw),
    )
}

fn missing(src: &str, expected: impl fmt::Display) -> anyhow::Error {
    let column = src.chars().count() + 1;
    syntax_error(
        src,
        column,
        format!("expected {expected}, found the end of the predicate"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parsed(src: &str) -> Predicate {
        match parse(src) {
            Ok(predicate) => predicate,
            Err(error) => panic!("{src:?} should parse: {error}"),
        }
    }

    fn error(src: &str) -> String {
        match parse(src) {
            Ok(predicate) => panic!("{src:?} should not parse, got {predicate:?}"),
            Err(error) => error.to_string(),
        }
    }

    fn evaluated(src: &str, snapshot: &Value) -> Evaluation {
        match eval(&parsed(src), snapshot) {
            Ok(evaluation) => evaluation,
            Err(error) => panic!("{src:?} should evaluate: {error}"),
        }
    }

    fn holds(src: &str, snapshot: &Value) -> bool {
        evaluated(src, snapshot).satisfied
    }

    fn snapshot() -> Value {
        json!({
            "version": 1,
            "screen": "Hub",
            "overlay": null,
            "lists": {
                "worktrees": {
                    "rows": [
                        {"id": "wt-1", "label": "main", "badges": ["dirty"]},
                        {"id": "wt-2", "label": "42", "badges": []},
                    ],
                    "selected": {"id": "wt-2", "label": "42"},
                    "filter": "",
                },
            },
            "toasts": [{"level": "error", "text": "job failed", "count": 3}],
            "terminal": {"text": "$ echo harness-ok\nharness-ok\n"},
            "window": {"scale_factor": 1.0, "frame": 12},
            "idle": {
                "idle": false,
                "in_flight_requests": 0,
                "running_jobs": 2,
                "pending_frame": false,
                "live_toast_timers": 0,
                "armed_debounces": 0,
            },
        })
    }

    /// `docs/TESTING-HARNESS.md` §3 freezes two snapshot keys that are not bare names — the
    /// `board.cards` list and every entry of the `targets` map — so a path grammar that cannot
    /// quote a segment publishes them where no predicate can reach them.
    #[test]
    fn a_quoted_segment_reaches_a_key_that_is_not_a_bare_name() {
        let snapshot = serde_json::json!({
            "lists": {
                "board.cards": { "rows": [{ "label": "Ship it" }] },
                "worktrees": { "rows": [] },
            },
            "targets": {
                "worktrees.row[0]": { "x": 12.0, "frame": 7 },
            },
        });

        for (path, expected) in [
            (
                r#"lists["board.cards"].rows[0].label"#,
                serde_json::json!("Ship it"),
            ),
            (r#"targets["worktrees.row[0]"].frame"#, serde_json::json!(7)),
            (r#"targets["worktrees.row[0]"].x"#, serde_json::json!(12.0)),
        ] {
            let resolved = resolve_str(path, &snapshot)
                .unwrap_or_else(|error| panic!("{path} should parse: {error}"));
            assert_eq!(resolved, Some(&expected), "{path} should resolve");

            // The whole predicate has to survive the lexer too: the quote that opens a path
            // step must not be read as the start of a value.
            let source = format!("{path} exists");
            let parsed = parse(&source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert!(
                eval(&parsed, &snapshot)
                    .unwrap_or_else(|error| panic!("{error}"))
                    .satisfied,
                "{source} should hold"
            );
        }

        // A quoted key that is not in the snapshot is still just a missing path.
        assert_eq!(
            resolve_str(r#"targets["nothing.here[9]"]"#, &snapshot)
                .unwrap_or_else(|error| panic!("{error}")),
            None
        );

        for (bad, reason) in [
            (r#"targets["unterminated"#, "unterminated quoted key"),
            (r#"targets[""]"#, "empty quoted key"),
            (r#"targets["a"x]"#, "unclosed `["),
        ] {
            let error = parse_path(bad).expect_err("should be rejected");
            assert!(
                error.contains(reason),
                "{bad:?} should report {reason:?}, got {error:?}"
            );
        }
    }

    #[test]
    fn parses_every_operator_and_renders_it_back() {
        for src in [
            "screen == \"Hub\"",
            "screen != \"Hub\"",
            "terminal.text ~= \"harness-ok\"",
            "toasts[0].count > 2",
            "toasts[0].count < 2.5",
            "toasts[0].count != true",
            "overlay == null",
            "overlay exists",
            "overlay absent",
            "idle",
        ] {
            assert_eq!(parsed(src).to_string(), src, "{src} should round-trip");
        }
    }

    #[test]
    fn parses_a_conjunction_of_every_atom_shape() {
        let predicate = parsed("idle && overlay absent && lists.worktrees.rows[0].id == wt-1");
        assert_eq!(
            predicate.clauses,
            vec![
                Clause::Idle,
                Clause::Absent {
                    path: "overlay".to_owned()
                },
                Clause::Compare {
                    path: "lists.worktrees.rows[0].id".to_owned(),
                    op: Operator::Eq,
                    value: json!("wt-1"),
                },
            ]
        );
    }

    #[test]
    fn reads_unquoted_values_as_the_json_scalars_they_look_like() {
        let cases = [
            ("a == 3", json!(3)),
            ("a == -7", json!(-7)),
            ("a == 1.5", json!(1.5)),
            ("a == true", json!(true)),
            ("a == false", json!(false)),
            ("a == null", json!(null)),
            ("a == Hub", json!("Hub")),
            ("a == harness-ok", json!("harness-ok")),
            ("a == NaN", json!("NaN")),
            ("a == -inf", json!("-inf")),
            ("a == \"3\"", json!("3")),
            ("a == \"two words\"", json!("two words")),
        ];
        for (src, expected) in cases {
            let Some(Clause::Compare { value, .. }) = parsed(src).clauses.first().cloned() else {
                panic!("{src} should parse to a comparison");
            };
            assert_eq!(value, expected, "{src}");
        }
    }

    #[test]
    fn keeps_regex_backslashes_and_quotes_intact() {
        let src = r#"a ~= "\d+\"x\\.y""#;
        let Some(Clause::Compare { value, .. }) = parsed(src).clauses.first().cloned() else {
            panic!("{src} should parse to a comparison");
        };
        assert_eq!(value, json!(r#"\d+"x\.y"#));
        assert!(holds(src, &json!({"a": r#"12"x.y"#})));
    }

    #[test]
    fn accepts_atoms_written_without_spaces() {
        assert_eq!(parsed("screen==Hub&&overlay exists").clauses.len(), 2);
    }

    #[test]
    fn rejects_malformed_predicates_naming_the_token_and_column() {
        let cases = [
            ("", "found the end of the predicate, at column 1"),
            ("   ", "expected a path or `idle`"),
            ("&& screen == Hub", "found `&&`, at column 1"),
            (
                "screen == Hub &&",
                "expected a path or `idle`, found the end",
            ),
            ("screen ==", "expected a value after `==`"),
            ("screen", "expected an operator after the path `screen`"),
            ("screen = Hub", "expected `==`, found `=`, at column 8"),
            ("screen ! Hub", "expected `!=`, found `!`, at column 8"),
            ("screen is Hub", "found `is`, at column 8"),
            ("screen == Hub Workspace", "found `Workspace`, at column 15"),
            ("screen == && x", "expected a value after `==`, found `&&`"),
            ("screen & overlay", "expected `&&`, found a single `&`"),
            ("a == \"open", "unterminated quoted value, at column 6"),
            ("a..b exists", "is an empty path segment"),
            (".a exists", "is an empty path segment"),
            ("a. exists", "is an empty path segment"),
            ("a[0 exists", "has an unclosed `[`"),
            ("a[x] exists", "is not an array index"),
            ("a[0]b exists", "has trailing text after `]`"),
            ("a$b exists", "is not allowed in the path segment"),
            ("a ~= \"[\"", "is not a valid regular expression"),
        ];
        for (src, expected) in cases {
            let message = error(src);
            assert!(
                message.contains(expected),
                "{src:?} should report {expected:?}, got {message:?}"
            );
        }
    }

    #[test]
    fn idle_is_a_path_when_it_is_followed_by_an_operator() {
        assert_eq!(parsed("idle").clauses, vec![Clause::Idle]);
        assert_eq!(
            parsed("idle.running_jobs > 0").clauses,
            vec![Clause::Compare {
                path: "idle.running_jobs".to_owned(),
                op: Operator::Greater,
                value: json!(0),
            }]
        );
        assert_eq!(
            parsed("idle exists").clauses,
            vec![Clause::Exists {
                path: "idle".to_owned()
            }]
        );
    }

    #[test]
    fn indexes_arrays_and_traverses_maps() {
        let snapshot = snapshot();
        assert!(holds("lists.worktrees.rows[1].label == 42", &snapshot));
        assert!(holds(
            "lists.worktrees.rows[0].badges[0] == dirty",
            &snapshot
        ));
        assert!(holds("lists.worktrees.selected.id == wt-2", &snapshot));
        assert!(holds("toasts[0].text == \"job failed\"", &snapshot));
    }

    #[test]
    fn a_missing_path_makes_every_comparison_false_and_reports_no_value() {
        let snapshot = snapshot();
        for src in [
            "dialog.name == Rename",
            "dialog.name != Rename",
            "toasts[9].count > 0",
            "toasts[9].count < 9",
            "dialog.name ~= \".*\"",
        ] {
            let evaluation = evaluated(src, &snapshot);
            assert!(!evaluation.satisfied, "{src} should be false");
            assert_eq!(evaluation.clauses[0].actual, None, "{src}");
        }
    }

    #[test]
    fn exists_and_absent_treat_null_as_absent() {
        let snapshot = snapshot();
        assert!(holds("overlay absent", &snapshot));
        assert!(!holds("overlay exists", &snapshot));
        assert!(holds("dialog absent", &snapshot));
        assert!(!holds("dialog exists", &snapshot));
        assert!(holds("screen exists", &snapshot));
        assert!(!holds("screen absent", &snapshot));
        assert!(holds("toasts exists", &snapshot));
    }

    #[test]
    fn compares_scalars_across_the_representations_a_scenario_can_write() {
        let snapshot = json!({
            "count": 3,
            "scale": 1.0,
            "label": "42",
            "flag": true,
            "text": "Hub",
        });
        for src in [
            "count == 3",
            "count == 3.0",
            "count != 4",
            "scale == 1",
            "label == 42",
            "label == \"42\"",
            "flag == true",
            "flag != false",
            "text == Hub",
            "text != Workspace",
        ] {
            assert!(holds(src, &snapshot), "{src} should hold");
        }
        for src in ["count == 4", "text == hub", "flag == false", "count != 3"] {
            assert!(!holds(src, &snapshot), "{src} should not hold");
        }
    }

    #[test]
    fn orders_numbers_and_refuses_to_order_anything_else() {
        let snapshot = snapshot();
        assert!(holds("toasts[0].count > 2", &snapshot));
        assert!(!holds("toasts[0].count > 3", &snapshot));
        assert!(holds("toasts[0].count < 4", &snapshot));
        assert!(!holds("toasts[0].count < 3", &snapshot));
        assert!(holds("lists.worktrees.rows[1].label > 41", &snapshot));
        assert!(!holds("screen > 1", &snapshot));
        assert!(!holds("screen < Zzz", &snapshot));
        assert!(!holds("overlay > 0", &snapshot));
    }

    #[test]
    fn matches_regular_expressions_against_scalar_text() {
        let snapshot = snapshot();
        assert!(holds("terminal.text ~= harness-ok", &snapshot));
        assert!(holds("terminal.text ~= \"^\\$ echo\"", &snapshot));
        assert!(holds("terminal.text ~= \"ok\\n\"", &snapshot));
        assert!(!holds("terminal.text ~= \"harness-nope\"", &snapshot));
        assert!(holds("toasts[0].count ~= \"^3$\"", &snapshot));
        assert!(!holds("overlay ~= \".*\"", &snapshot));
        assert!(!holds("toasts ~= \".*\"", &snapshot));
    }

    #[test]
    fn idle_reads_idle_idle_and_reports_the_whole_counter_block() {
        let mut snapshot = snapshot();
        let evaluation = evaluated("idle", &snapshot);
        assert!(!evaluation.satisfied);
        assert_eq!(
            evaluation.clauses[0]
                .actual
                .as_ref()
                .and_then(|v| v.get("running_jobs")),
            Some(&json!(2))
        );
        assert!(
            evaluation
                .failure_summary()
                .is_some_and(|s| s.contains("running_jobs"))
        );

        snapshot["idle"]["idle"] = json!(true);
        assert!(holds("idle", &snapshot));
        assert!(holds("idle && screen == Hub", &snapshot));
        assert_eq!(evaluated("idle", &json!({})).clauses[0].actual, None);
    }

    #[test]
    fn a_conjunction_evaluates_every_clause_and_reports_each_actual_value() {
        let snapshot = snapshot();
        let evaluation = evaluated("screen == Workspace && overlay == Help", &snapshot);
        assert!(!evaluation.satisfied);
        assert_eq!(evaluation.clauses.len(), 2);
        assert_eq!(evaluation.clauses[0].actual, Some(json!("Hub")));
        assert_eq!(evaluation.clauses[1].actual, Some(json!(null)));
        assert_eq!(evaluation.failures().count(), 2);
        let summary = evaluation.failure_summary().unwrap_or_default();
        assert!(
            summary.contains("`screen == \"Workspace\"` failed; screen is \"Hub\""),
            "{summary}"
        );
        assert!(
            summary.contains("`overlay == \"Help\"` failed; overlay is null"),
            "{summary}"
        );
    }

    #[test]
    fn a_satisfied_predicate_has_no_failure_summary() {
        let evaluation = evaluated("screen == Hub && toasts[0].count > 1", &snapshot());
        assert!(evaluation.satisfied);
        assert_eq!(evaluation.failure_summary(), None);
        assert_eq!(evaluation.failures().count(), 0);
    }

    #[test]
    fn describes_a_missing_path_as_missing() {
        let evaluation = evaluated("dialog.name == Rename", &snapshot());
        assert_eq!(
            evaluation.clauses[0].describe(),
            "`dialog.name == \"Rename\"` failed; dialog.name is missing from the snapshot"
        );
    }

    #[test]
    fn a_hand_built_predicate_with_a_bad_path_or_pattern_is_an_evaluation_error() {
        let bad_path = Predicate {
            clauses: vec![Clause::Exists {
                path: "a..b".to_owned(),
            }],
        };
        assert!(eval(&bad_path, &snapshot()).is_err());

        let bad_pattern = Predicate {
            clauses: vec![Clause::Compare {
                path: "screen".to_owned(),
                op: Operator::Regex,
                value: json!("["),
            }],
        };
        assert!(eval(&bad_pattern, &snapshot()).is_err());
    }

    #[test]
    fn round_trips_through_serde_so_the_runner_and_app_agree() {
        let predicate = parsed("idle && toasts[0].count > 2 && overlay absent");
        let json = match serde_json::to_string(&predicate) {
            Ok(json) => json,
            Err(error) => panic!("predicate should serialize: {error}"),
        };
        let back: Predicate = match serde_json::from_str(&json) {
            Ok(back) => back,
            Err(error) => panic!("predicate should deserialize: {error}"),
        };
        assert_eq!(back, predicate);
    }
}
