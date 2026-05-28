//! Hand-written parser for the DOT subset described in
//! `specs/core/p1-dot-parser.spec.md`.

use std::collections::BTreeMap;

use super::ast::{Edge, Graph, Node};
use crate::CoreError;

/// Parse a DOT-subset source into a typed [`Graph`].
///
/// # Errors
/// Returns [`CoreError::DotParse`] for any token outside the documented subset.
pub fn parse(src: &str) -> Result<Graph, CoreError> {
    let mut lexer = Lexer::new(src);
    parse_digraph(&mut lexer)
}

// ─── Lexer ──────────────────────────────────────────────────────────

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    peeked: Option<Tok>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    QuotedString(String),
    Arrow,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Eq,
    Comma,
    Semi,
    Eof,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            peeked: None,
        }
    }

    fn peek_tok(&mut self) -> Result<&Tok, CoreError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.read_one()?);
        }
        Ok(self.peeked.as_ref().expect("peeked just set"))
    }

    fn next_tok(&mut self) -> Result<Tok, CoreError> {
        if let Some(t) = self.peeked.take() {
            return Ok(t);
        }
        self.read_one()
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.src[self.pos..].starts_with("//") {
                while let Some(c) = self.bump() {
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn read_one(&mut self) -> Result<Tok, CoreError> {
        self.skip_ws_and_comments();
        let Some(c) = self.peek() else {
            return Ok(Tok::Eof);
        };

        match c {
            '{' => {
                self.bump();
                Ok(Tok::LBrace)
            }
            '}' => {
                self.bump();
                Ok(Tok::RBrace)
            }
            '[' => {
                self.bump();
                Ok(Tok::LBracket)
            }
            ']' => {
                self.bump();
                Ok(Tok::RBracket)
            }
            '=' => {
                self.bump();
                Ok(Tok::Eq)
            }
            ',' => {
                self.bump();
                Ok(Tok::Comma)
            }
            ';' => {
                self.bump();
                Ok(Tok::Semi)
            }
            '-' => {
                self.bump();
                if self.peek() == Some('>') {
                    self.bump();
                    Ok(Tok::Arrow)
                } else {
                    Err(CoreError::DotParse(format!(
                        "unexpected '-' at pos {}",
                        self.pos
                    )))
                }
            }
            '"' => self.read_quoted(),
            c if c.is_ascii_alphanumeric() || c == '_' || c == '.' => Ok(self.read_ident()),
            other => Err(CoreError::DotParse(format!(
                "unexpected char {other:?} at pos {}",
                self.pos
            ))),
        }
    }

    // Infallible: an ident is whatever run of ident-chars follows; never errors.
    fn read_ident(&mut self) -> Tok {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                self.bump();
            } else {
                break;
            }
        }
        Tok::Ident(self.src[start..self.pos].to_string())
    }

    fn read_quoted(&mut self) -> Result<Tok, CoreError> {
        self.bump(); // opening "
        let mut out = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(CoreError::DotParse("unterminated quoted string".into()));
            };
            if c == '"' {
                return Ok(Tok::QuotedString(out));
            }
            if c == '\\' {
                let Some(esc) = self.bump() else {
                    return Err(CoreError::DotParse(
                        "trailing backslash in quoted string".into(),
                    ));
                };
                match esc {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    other => {
                        return Err(CoreError::DotParse(format!("unknown escape \\{other}")));
                    }
                }
            } else {
                out.push(c);
            }
        }
    }
}

// ─── Parser ─────────────────────────────────────────────────────────

fn parse_digraph(lex: &mut Lexer) -> Result<Graph, CoreError> {
    // expect: 'digraph'
    match lex.next_tok()? {
        Tok::Ident(kw) if kw == "digraph" => {}
        other => {
            return Err(CoreError::DotParse(format!(
                "expected 'digraph', got {other:?}"
            )));
        }
    }

    // optional name (may be elided: `digraph { ... }`)
    let name = match lex.peek_tok()? {
        Tok::LBrace => String::new(),
        _ => match lex.next_tok()? {
            Tok::Ident(s) | Tok::QuotedString(s) => s,
            other => {
                return Err(CoreError::DotParse(format!(
                    "expected graph name, got {other:?}"
                )));
            }
        },
    };

    // require `{`
    match lex.next_tok()? {
        Tok::LBrace => {}
        other => return Err(CoreError::DotParse(format!("expected '{{', got {other:?}"))),
    }

    let mut graph = Graph {
        name,
        ..Default::default()
    };

    loop {
        let tok = lex.next_tok()?;
        match tok {
            Tok::RBrace => return Ok(graph),
            Tok::Eof => return Err(CoreError::DotParse("unexpected EOF inside graph".into())),
            Tok::Semi => {} // stray separator between statements
            Tok::Ident(s) if s == "subgraph" => {
                return Err(CoreError::DotParse(
                    "subgraph not supported in v0; see roadmap".into(),
                ));
            }
            Tok::Ident(id) | Tok::QuotedString(id) => {
                parse_stmt_after_id(lex, &mut graph, id)?;
            }
            other => return Err(CoreError::DotParse(format!("unexpected token {other:?}"))),
        }
    }
}

fn parse_stmt_after_id(lex: &mut Lexer, graph: &mut Graph, id: String) -> Result<(), CoreError> {
    // peek decides node vs edge vs bare-id
    match lex.peek_tok()? {
        Tok::Arrow => {
            let _ = lex.next_tok()?; // consume '->'
            let to = match lex.next_tok()? {
                Tok::Ident(s) | Tok::QuotedString(s) => s,
                other => {
                    return Err(CoreError::DotParse(format!(
                        "expected target id after '->', got {other:?}"
                    )));
                }
            };
            let attrs = maybe_attrs(lex)?;
            graph.edges.push(Edge {
                from: id,
                to,
                attrs,
            });
        }
        Tok::LBracket => {
            let _ = lex.next_tok()?; // consume '['
            let attrs = parse_attr_list_body(lex)?;
            graph.nodes.push(Node { id, attrs });
        }
        _ => {
            graph.nodes.push(Node {
                id,
                attrs: BTreeMap::new(),
            });
        }
    }
    consume_optional_semi(lex)?;
    Ok(())
}

fn maybe_attrs(lex: &mut Lexer) -> Result<BTreeMap<String, String>, CoreError> {
    if matches!(lex.peek_tok()?, Tok::LBracket) {
        let _ = lex.next_tok()?; // consume '['
        parse_attr_list_body(lex)
    } else {
        Ok(BTreeMap::new())
    }
}

fn parse_attr_list_body(lex: &mut Lexer) -> Result<BTreeMap<String, String>, CoreError> {
    let mut attrs = BTreeMap::new();
    loop {
        let tok = lex.next_tok()?;
        match tok {
            Tok::RBracket => return Ok(attrs),
            Tok::Comma => {} // separator between attributes
            Tok::Ident(k) | Tok::QuotedString(k) => {
                match lex.next_tok()? {
                    Tok::Eq => {}
                    other => {
                        return Err(CoreError::DotParse(format!(
                            "expected '=' after attr name {k}, got {other:?}"
                        )));
                    }
                }
                let v = match lex.next_tok()? {
                    Tok::Ident(s) | Tok::QuotedString(s) => s,
                    other => {
                        return Err(CoreError::DotParse(format!(
                            "expected attr value, got {other:?}"
                        )));
                    }
                };
                attrs.insert(k, v);
            }
            other => {
                return Err(CoreError::DotParse(format!(
                    "unexpected token in attr list: {other:?}"
                )));
            }
        }
    }
}

fn consume_optional_semi(lex: &mut Lexer) -> Result<(), CoreError> {
    if matches!(lex.peek_tok()?, Tok::Semi) {
        let _ = lex.next_tok()?;
    }
    Ok(())
}
