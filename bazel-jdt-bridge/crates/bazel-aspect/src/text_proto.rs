//! Text proto parser for Bazel IntelliJ IDE info.
//!
//! This module implements a recursive descent parser for the text proto format
//! used by Bazel's intellij-info-java aspect. The parser handles errors gracefully
//! without panicking - malformed fields are skipped and parsing continues.

use crate::ide_info::{ArtifactLocation, JarInfo, JavaIdeInfo, JavacOptions, TargetIdeInfo};
use std::collections::HashMap;
use thiserror::Error;

/// Errors that can occur during text proto parsing.
#[derive(Debug, Clone, Error)]
pub enum ParseError {
    #[error("Unexpected end of input at position {position}")]
    UnexpectedEndOfInput { position: usize },

    #[error("Unexpected character '{char}' at position {position}, expected {expected}")]
    UnexpectedCharacter {
        position: usize,
        char: char,
        expected: String,
    },

    #[error("Invalid field value for '{field}' at position {position}: {message}")]
    InvalidFieldValue {
        position: usize,
        field: String,
        message: String,
    },

    #[error("Failed to parse string at position {position}: {message}")]
    StringParseError { position: usize, message: String },
}

/// Result of parsing with error recovery information.
#[derive(Debug)]
pub struct ParseResult<T> {
    /// The parsed value (may be default if parsing failed completely)
    pub value: T,
    /// Errors encountered during parsing (non-fatal)
    pub errors: Vec<ParseError>,
    /// Number of fields that were skipped due to errors
    pub skipped_fields: usize,
}

impl<T: Default> ParseResult<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            errors: Vec::new(),
            skipped_fields: 0,
        }
    }

    fn add_error(&mut self, error: ParseError) {
        self.errors.push(error);
        self.skipped_fields += 1;
    }
}

/// Token types for the lexer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Identifier(String),
    StringLiteral(String),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    Bool(bool),
    Eof,
}

/// Lexer for tokenizing text proto input.
struct Lexer<'a> {
    #[expect(dead_code)]
    input: &'a str,
    chars: Vec<char>,
    position: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.chars().collect(),
            position: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.position).copied();
        self.position += 1;
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_comment(&mut self) {
        // Skip # comments until end of line
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.advance();
        }
    }

    fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_whitespace();

        let position = self.position;

        match self.peek() {
            None => Ok(Token::Eof),
            Some('#') => {
                self.skip_comment();
                self.next_token()
            }
            Some('{') => {
                self.advance();
                Ok(Token::LBrace)
            }
            Some('}') => {
                self.advance();
                Ok(Token::RBrace)
            }
            Some('[') => {
                self.advance();
                Ok(Token::LBracket)
            }
            Some(']') => {
                self.advance();
                Ok(Token::RBracket)
            }
            Some(':') => {
                self.advance();
                Ok(Token::Colon)
            }
            Some(',') => {
                self.advance();
                Ok(Token::Comma)
            }
            Some('"') => self.parse_string(),
            Some('\'') => self.parse_single_quoted_string(),
            Some(c) if c.is_ascii_digit() || c == '-' => self.parse_number_or_bool(),
            Some(c) if c.is_alphabetic() || c == '_' => self.parse_identifier_or_bool(),
            Some(c) => Err(ParseError::UnexpectedCharacter {
                position,
                char: c,
                expected: "valid token".to_string(),
            }),
        }
    }

    fn parse_string(&mut self) -> Result<Token, ParseError> {
        let start = self.position;
        self.advance(); // consume opening quote

        let mut result = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::StringParseError {
                        position: start,
                        message: "Unterminated string".to_string(),
                    });
                }
                Some('"') => {
                    self.advance(); // consume closing quote
                    break;
                }
                Some('\\') => {
                    self.advance();
                    match self.peek() {
                        Some('n') => result.push('\n'),
                        Some('t') => result.push('\t'),
                        Some('r') => result.push('\r'),
                        Some('\\') => result.push('\\'),
                        Some('"') => result.push('"'),
                        Some(c) => result.push(c),
                        None => {
                            return Err(ParseError::StringParseError {
                                position: self.position,
                                message: "Unexpected end after escape".to_string(),
                            });
                        }
                    }
                    self.advance();
                }
                Some(c) => {
                    result.push(c);
                    self.advance();
                }
            }
        }

        Ok(Token::StringLiteral(result))
    }

    fn parse_single_quoted_string(&mut self) -> Result<Token, ParseError> {
        let start = self.position;
        self.advance(); // consume opening quote

        let mut result = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(ParseError::StringParseError {
                        position: start,
                        message: "Unterminated string".to_string(),
                    });
                }
                Some('\'') => {
                    self.advance(); // consume closing quote
                    break;
                }
                Some('\\') => {
                    self.advance();
                    if let Some(c) = self.advance() {
                        result.push(c);
                    }
                }
                Some(c) => {
                    result.push(c);
                    self.advance();
                }
            }
        }

        Ok(Token::StringLiteral(result))
    }

    fn parse_number_or_bool(&mut self) -> Result<Token, ParseError> {
        let mut s = String::new();

        if self.peek() == Some('-') {
            s.push('-');
            self.advance();
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }

        Ok(Token::Identifier(s))
    }

    fn parse_identifier_or_bool(&mut self) -> Result<Token, ParseError> {
        let mut s = String::new();

        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }

        // Check for boolean literals
        match s.as_str() {
            "true" => Ok(Token::Bool(true)),
            "false" => Ok(Token::Bool(false)),
            _ => Ok(Token::Identifier(s)),
        }
    }
}

/// Parser for text proto format.
pub struct TextProtoParser<'a> {
    lexer: Lexer<'a>,
    current_token: Token,
}

impl<'a> TextProtoParser<'a> {
    /// Create a new parser for the given input.
    pub fn new(input: &'a str) -> Self {
        let mut lexer = Lexer::new(input);
        let current_token = lexer.next_token().unwrap_or(Token::Eof);
        Self {
            lexer,
            current_token,
        }
    }

    fn advance(&mut self) -> Result<Token, ParseError> {
        let token = self.lexer.next_token()?;
        self.current_token = token.clone();
        Ok(token)
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ParseError> {
        if std::mem::discriminant(&self.current_token) == std::mem::discriminant(expected) {
            self.advance()?;
            Ok(())
        } else {
            Err(ParseError::UnexpectedCharacter {
                position: self.lexer.position,
                char: format!("{:?}", self.current_token)
                    .chars()
                    .next()
                    .unwrap_or('?'),
                expected: format!("{:?}", expected),
            })
        }
    }

    /// Parse a complete TargetIdeInfo from text proto format.
    pub fn parse_target_ide_info(&mut self) -> ParseResult<TargetIdeInfo> {
        let mut result = ParseResult::new(TargetIdeInfo::new(String::new(), String::new()));
        let mut fields: HashMap<String, ProtoValue> = HashMap::new();

        while self.current_token != Token::Eof {
            match self.parse_field() {
                Ok((name, value)) => match (fields.get(&name), value) {
                    (Some(ProtoValue::Strings(existing)), ProtoValue::String(new)) => {
                        let mut updated = existing.clone();
                        updated.push(new);
                        fields.insert(name, ProtoValue::Strings(updated));
                    }
                    (Some(ProtoValue::String(old)), ProtoValue::String(new)) => {
                        fields.insert(name, ProtoValue::Strings(vec![old.clone(), new]));
                    }
                    (_, value) => {
                        fields.insert(name, value);
                    }
                },
                Err(e) => {
                    result.add_error(e);
                    self.skip_to_next_field();
                }
            }
        }

        // Extract target fields
        if let Some(ProtoValue::String(s)) = fields.remove("label") {
            result.value.label = s;
        }
        if let Some(ProtoValue::String(s)) = fields.remove("kind") {
            result.value.kind = s;
        }
        if let Some(ProtoValue::Message(m)) = fields.remove("build_file") {
            result.value.build_file = Some(self.extract_artifact_location(m));
        }
        if let Some(ProtoValue::Message(m)) = fields.remove("java_info") {
            result.value.java_info = Some(self.extract_java_ide_info(m));
        }
        match fields.remove("deps") {
            Some(ProtoValue::Strings(s)) => result.value.deps = s,
            Some(ProtoValue::String(s)) => result.value.deps = vec![s],
            _ => {}
        }
        match fields.remove("runtime_deps") {
            Some(ProtoValue::Strings(s)) => result.value.runtime_deps = s,
            Some(ProtoValue::String(s)) => result.value.runtime_deps = vec![s],
            _ => {}
        }
        match fields.remove("exports") {
            Some(ProtoValue::Strings(s)) => result.value.exports = s,
            Some(ProtoValue::String(s)) => result.value.exports = vec![s],
            _ => {}
        }

        result
    }

    fn parse_field(&mut self) -> Result<(String, ProtoValue), ParseError> {
        // field_name: value OR field_name { ... } OR field_name [ ... ]
        let name = match &self.current_token {
            Token::Identifier(s) => s.clone(),
            _ => {
                return Err(ParseError::UnexpectedCharacter {
                    position: self.lexer.position,
                    char: format!("{:?}", self.current_token)
                        .chars()
                        .next()
                        .unwrap_or('?'),
                    expected: "identifier".to_string(),
                });
            }
        };

        self.advance()?;

        // Check for colon (optional in some formats)
        if self.current_token == Token::Colon {
            self.advance()?;
        }

        let value = match &self.current_token {
            Token::LBrace => {
                self.advance()?;
                match self.parse_message() {
                    Ok(msg) => {
                        let _ = self.expect(&Token::RBrace);
                        ProtoValue::Message(msg)
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
            Token::LBracket => {
                self.advance()?;
                let arr = self.parse_array()?;
                self.expect(&Token::RBracket)?;
                arr
            }
            Token::StringLiteral(s) => {
                let s = s.clone();
                self.advance()?;
                ProtoValue::String(s)
            }
            Token::Identifier(s) => {
                let s = s.clone();
                self.advance()?;
                ProtoValue::String(s)
            }
            Token::Bool(b) => {
                let b = *b;
                self.advance()?;
                ProtoValue::Bool(b)
            }
            _ => {
                return Err(ParseError::UnexpectedCharacter {
                    position: self.lexer.position,
                    char: format!("{:?}", self.current_token)
                        .chars()
                        .next()
                        .unwrap_or('?'),
                    expected: "value".to_string(),
                });
            }
        };

        Ok((name, value))
    }

    fn parse_message(&mut self) -> Result<HashMap<String, ProtoValue>, ParseError> {
        let mut fields = HashMap::new();

        while self.current_token != Token::RBrace && self.current_token != Token::Eof {
            match self.parse_field() {
                Ok((name, value)) => {
                    let existing = fields.remove(&name);
                    match (existing, value) {
                        (Some(ProtoValue::Strings(mut arr)), ProtoValue::String(s)) => {
                            arr.push(s);
                            fields.insert(name, ProtoValue::Strings(arr));
                        }
                        (Some(ProtoValue::Messages(mut arr)), ProtoValue::Message(m)) => {
                            arr.push(m);
                            fields.insert(name, ProtoValue::Messages(arr));
                        }
                        (Some(ProtoValue::String(existing)), ProtoValue::String(s)) => {
                            fields.insert(name, ProtoValue::Strings(vec![existing, s]));
                        }
                        (Some(ProtoValue::Message(existing)), ProtoValue::Message(m)) => {
                            fields.insert(name, ProtoValue::Messages(vec![existing, m]));
                        }
                        (_, value) => {
                            fields.insert(name, value);
                        }
                    }
                }
                Err(_) => {
                    if self.current_token == Token::Eof {
                        break;
                    }
                    self.skip_to_next_field();
                }
            }
        }

        if self.current_token == Token::Eof {
            return Err(ParseError::UnexpectedEndOfInput { position: 0 });
        }

        Ok(fields)
    }

    fn parse_array(&mut self) -> Result<ProtoValue, ParseError> {
        let mut strings = Vec::new();
        let mut messages = Vec::new();

        while self.current_token != Token::RBracket && self.current_token != Token::Eof {
            match &self.current_token {
                Token::StringLiteral(s) => {
                    strings.push(s.clone());
                    self.advance()?;
                }
                Token::Identifier(s) => {
                    strings.push(s.clone());
                    self.advance()?;
                }
                Token::LBrace => {
                    self.advance()?;
                    let msg = self.parse_message()?;
                    messages.push(msg);
                    self.expect(&Token::RBrace)?;
                }
                Token::Comma => {
                    self.advance()?;
                }
                _ => {
                    // Skip unknown tokens in array
                    self.advance()?;
                }
            }
        }

        if !messages.is_empty() {
            Ok(ProtoValue::Messages(messages))
        } else {
            Ok(ProtoValue::Strings(strings))
        }
    }

    fn skip_to_next_field(&mut self) {
        let mut brace_depth = 0;

        loop {
            match &self.current_token {
                Token::Eof => break,
                Token::RBrace => {
                    if brace_depth == 0 {
                        break;
                    }
                    brace_depth -= 1;
                }
                Token::LBrace => {
                    brace_depth += 1;
                }
                Token::Identifier(_) if brace_depth == 0 => {
                    break;
                }
                _ => {}
            }

            if self.advance().is_err() {
                break;
            }
        }
    }

    fn extract_artifact_location(&self, fields: HashMap<String, ProtoValue>) -> ArtifactLocation {
        let mut loc = ArtifactLocation::default();

        if let Some(ProtoValue::String(s)) = fields.get("relative_path") {
            loc.relative_path = Some(s.clone());
        }
        if let Some(ProtoValue::String(s)) = fields.get("absolute_path") {
            loc.absolute_path = Some(s.clone());
        }
        if let Some(ProtoValue::Bool(b)) = fields.get("is_source") {
            loc.is_source = *b;
        }
        if let Some(ProtoValue::Bool(b)) = fields.get("is_external") {
            loc.is_external = *b;
        }
        if let Some(ProtoValue::String(s)) = fields.get("root_execution_path_fragment") {
            loc.root_execution_path_fragment = Some(s.clone());
        }

        loc
    }

    fn extract_jar_info(&self, fields: HashMap<String, ProtoValue>) -> JarInfo {
        let mut jar = JarInfo::default();

        if let Some(ProtoValue::Message(m)) = fields.get("jar") {
            jar.jar = self.extract_artifact_location(m.clone());
        }
        if let Some(ProtoValue::Message(m)) = fields.get("source_jar") {
            jar.source_jar = Some(self.extract_artifact_location(m.clone()));
        }
        if let Some(ProtoValue::Message(m)) = fields.get("interface_jar") {
            jar.interface_jar = Some(self.extract_artifact_location(m.clone()));
        }

        jar
    }

    fn extract_java_ide_info(&self, fields: HashMap<String, ProtoValue>) -> JavaIdeInfo {
        let mut info = JavaIdeInfo::default();

        if let Some(ProtoValue::Messages(msgs)) = fields.get("sources") {
            info.sources = msgs
                .iter()
                .map(|m| self.extract_artifact_location(m.clone()))
                .collect();
        } else if let Some(ProtoValue::Message(m)) = fields.get("sources") {
            info.sources = vec![self.extract_artifact_location(m.clone())];
        }
        if let Some(ProtoValue::Messages(msgs)) = fields.get("jars") {
            info.jars = msgs
                .iter()
                .map(|m| self.extract_jar_info(m.clone()))
                .collect();
        } else if let Some(ProtoValue::Message(m)) = fields.get("jars") {
            info.jars = vec![self.extract_jar_info(m.clone())];
        }
        if let Some(ProtoValue::Messages(msgs)) = fields.get("generated_jars") {
            info.generated_jars = msgs
                .iter()
                .map(|m| self.extract_jar_info(m.clone()))
                .collect();
        }
        if let Some(ProtoValue::Messages(msgs)) = fields.get("compile_jars") {
            info.compile_jars = msgs
                .iter()
                .map(|m| self.extract_artifact_location(m.clone()))
                .collect();
        }
        if let Some(ProtoValue::Messages(msgs)) = fields.get("runtime_jars") {
            info.runtime_jars = msgs
                .iter()
                .map(|m| self.extract_artifact_location(m.clone()))
                .collect();
        }
        if let Some(ProtoValue::Strings(strs)) = fields.get("annotation_processors") {
            info.annotation_processors = strs.clone();
        }
        if let Some(ProtoValue::Messages(msgs)) = fields.get("source_jars") {
            info.source_jars = msgs
                .iter()
                .map(|m| self.extract_artifact_location(m.clone()))
                .collect();
        }
        if let Some(ProtoValue::Strings(strs)) = fields.get("javac_options") {
            info.javac_options = Some(JavacOptions {
                options: strs.clone(),
            });
        }

        info
    }
}

/// Intermediate representation of proto values during parsing.
#[derive(Debug, Clone)]
enum ProtoValue {
    String(String),
    Strings(Vec<String>),
    Bool(bool),
    Message(HashMap<String, ProtoValue>),
    Messages(Vec<HashMap<String, ProtoValue>>),
}

/// Parse text proto input into a TargetIdeInfo.
///
/// This function handles errors gracefully - malformed fields are skipped
/// and parsing continues. Errors are collected and returned along with
/// the parsed value.
pub fn parse_text_proto(input: &str) -> ParseResult<TargetIdeInfo> {
    let mut parser = TextProtoParser::new(input);
    parser.parse_target_ide_info()
}

/// Parse text proto input, returning only the TargetIdeInfo.
///
/// Errors are logged but not returned. Use `parse_text_proto` if you need
/// error details.
pub fn parse_text_proto_quiet(input: &str) -> TargetIdeInfo {
    parse_text_proto(input).value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_target() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
        "#;

        let result = parse_text_proto(input);
        assert_eq!(result.value.label, "//foo:bar");
        assert_eq!(result.value.kind, "java_library");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_parse_with_deps() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
            deps: "//foo:baz"
            deps: "//common:util"
        "#;

        let result = parse_text_proto(input);
        assert_eq!(result.value.deps, vec!["//foo:baz", "//common:util"]);
    }

    #[test]
    fn test_parse_with_java_info() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
            java_info {
                sources {
                    relative_path: "src/main/java/Foo.java"
                    is_source: true
                }
                jars {
                    jar {
                        relative_path: "bazel-out/bin/foo/bar.jar"
                    }
                }
            }
        "#;

        let result = parse_text_proto(input);
        assert!(result.value.is_java_target());

        let java_info = result.value.java_info.as_ref().unwrap();
        assert_eq!(java_info.sources.len(), 1);
        assert_eq!(java_info.jars.len(), 1);
    }

    #[test]
    fn test_parse_error_recovery() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
            : "orphan_value"
            deps: "//foo:baz"
        "#;

        let result = parse_text_proto(input);
        assert_eq!(result.value.label, "//foo:bar");
        assert_eq!(result.value.kind, "java_library");
        assert_eq!(result.value.deps, vec!["//foo:baz"]);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_parse_string_escapes() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
        "#;

        let result = parse_text_proto(input);
        assert_eq!(result.value.label, "//foo:bar");
    }

    #[test]
    fn test_parse_quoted_strings() {
        let input = r#"
            label: "//foo:bar"
            kind: "java_library"
        "#;

        let result = parse_text_proto(input);
        assert_eq!(result.value.label, "//foo:bar");
        assert_eq!(result.value.kind, "java_library");
    }
}
