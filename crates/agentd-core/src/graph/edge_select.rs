//! Edge-selection priority (design §2.4 + D8b) and the `condition` mini-language.
//! See `specs/core/p3-outcome-and-edge-selection.spec.md`.

use std::collections::{BTreeSet, HashMap};

use crate::graph::node_graph::{EdgeDef, NodeGraph};
use crate::types::{Outcome, RunContext, Status};

/// Pick the next edge out of `node_id` per the priority tiers. `attempts` maps a
/// node id to how many times it has run (for the `retry_target` ceiling).
#[must_use]
pub fn select_next_edge<'g, S: std::hash::BuildHasher>(
    graph: &'g NodeGraph,
    node_id: &str,
    outcome: &Outcome,
    ctx: &RunContext,
    attempts: &HashMap<String, u32, S>,
) -> Option<&'g EdgeDef> {
    let outgoing: Vec<&EdgeDef> = graph.edges.iter().filter(|e| e.from == node_id).collect();
    if outgoing.is_empty() {
        return None;
    }

    // 1. condition match (source order)
    for e in &outgoing {
        if let Some(c) = e.attr("condition")
            && eval_condition(c, outcome, ctx)
        {
            return Some(e);
        }
    }

    // 2. handler-suggested preferred_label
    if let Some(lbl) = outcome.preferred_label.as_deref()
        && let Some(e) = outgoing.iter().find(|e| e.attr("label") == Some(lbl))
    {
        return Some(e);
    }

    // 3. retry_target on Fail, only under the target's attempt ceiling
    if outcome.status == Status::Fail {
        for e in &outgoing {
            if e.is_retry_target() {
                let max = graph
                    .node(&e.to)
                    .and_then(|n| n.attr("retry_policy"))
                    .map_or(0, parse_max);
                let used = attempts.get(&e.to).copied().unwrap_or(0);
                if used < max {
                    return Some(e);
                }
            }
        }
    }

    // 4. handler-suggested next ids
    if !outcome.suggested_next_ids.is_empty() {
        let suggested: BTreeSet<&str> = outcome
            .suggested_next_ids
            .iter()
            .map(crate::types::NodeId::as_str)
            .collect();
        if let Some(e) = outgoing.iter().find(|e| suggested.contains(e.to.as_str())) {
            return Some(e);
        }
    }

    // 5 + 6. unconditional edges: highest weight, then lexical tiebreak on target
    let mut unconditional: Vec<&&EdgeDef> = outgoing
        .iter()
        .filter(|e| e.attr("condition").is_none())
        .collect();
    unconditional.sort_by(|a, b| weight(b).cmp(&weight(a)).then_with(|| a.to.cmp(&b.to)));
    unconditional.first().map(|e| **e)
}

fn weight(e: &EdgeDef) -> i64 {
    e.attr("weight").and_then(|w| w.parse().ok()).unwrap_or(1)
}

/// Parse `max=N` out of a `retry_policy="max=3,backoff=exp"` value.
fn parse_max(policy: &str) -> u32 {
    policy
        .split(',')
        .find_map(|part| part.trim().strip_prefix("max="))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

// ─── condition mini-language ────────────────────────────────────────

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Success => "success",
        Status::Fail => "fail",
        Status::Retry => "retry",
        Status::PartialSuccess => "partial_success",
    }
}

/// Evaluate a boolean condition expression against an Outcome + context.
/// Grammar: `or := and ("||" and)*`, `and := not ("&&" not)*`,
/// `not := "!" not | primary`, `primary := "(" or ")" | atom`,
/// `atom := outcome=<id> | answer=<id> | kv("k")=="v"`.
/// Any parse failure evaluates to `false` (an unparseable condition never routes).
#[must_use]
pub fn eval_condition(src: &str, outcome: &Outcome, ctx: &RunContext) -> bool {
    let tokens = lex(src);
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        outcome,
        ctx,
        failed: false,
    };
    let v = p.parse_or();
    // Require full, well-formed consumption: trailing junk leaves leftover tokens
    // (pos != len); a malformed sub-expression (e.g. an unbalanced open paren)
    // sets `failed`. Either way an unparseable condition evaluates to false and
    // never routes.
    if p.failed || p.pos != p.tokens.len() {
        false
    } else {
        v
    }
}

#[derive(Debug, Clone, PartialEq)]
enum CTok {
    Ident(String),
    Str(String),
    Eq,
    EqEq,
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn lex(src: &str) -> Vec<CTok> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push(CTok::LParen);
                i += 1;
            }
            ')' => {
                out.push(CTok::RParen);
                i += 1;
            }
            '!' => {
                out.push(CTok::Not);
                i += 1;
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                out.push(CTok::And);
                i += 2;
            }
            '|' if chars.get(i + 1) == Some(&'|') => {
                out.push(CTok::Or);
                i += 2;
            }
            '=' if chars.get(i + 1) == Some(&'=') => {
                out.push(CTok::EqEq);
                i += 2;
            }
            '=' => {
                out.push(CTok::Eq);
                i += 1;
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                i += 1; // closing quote
                out.push(CTok::Str(s));
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' => {
                let mut s = String::new();
                while i < chars.len()
                    && (chars[i].is_alphanumeric()
                        || chars[i] == '_'
                        || chars[i] == '.'
                        || chars[i] == '-')
                {
                    s.push(chars[i]);
                    i += 1;
                }
                out.push(CTok::Ident(s));
            }
            _ => i += 1, // skip anything unexpected
        }
    }
    out
}

struct Parser<'a> {
    tokens: &'a [CTok],
    pos: usize,
    outcome: &'a Outcome,
    ctx: &'a RunContext,
    /// Set when a sub-expression is malformed (e.g. an unbalanced `(`); forces
    /// `eval_condition` to return false so a broken condition never routes.
    failed: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&CTok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<&CTok> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> bool {
        let mut v = self.parse_and();
        while self.peek() == Some(&CTok::Or) {
            self.pos += 1;
            let rhs = self.parse_and();
            v = v || rhs;
        }
        v
    }

    fn parse_and(&mut self) -> bool {
        let mut v = self.parse_not();
        while self.peek() == Some(&CTok::And) {
            self.pos += 1;
            let rhs = self.parse_not();
            v = v && rhs;
        }
        v
    }

    fn parse_not(&mut self) -> bool {
        if self.peek() == Some(&CTok::Not) {
            self.pos += 1;
            return !self.parse_not();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> bool {
        if self.peek() == Some(&CTok::LParen) {
            self.pos += 1;
            let v = self.parse_or();
            if self.peek() == Some(&CTok::RParen) {
                self.pos += 1;
            } else {
                // Unbalanced open paren — the consumption guard can't catch this
                // (no leftover token), so mark the parse failed explicitly.
                self.failed = true;
            }
            return v;
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> bool {
        let Some(CTok::Ident(head)) = self.bump().cloned() else {
            return false;
        };
        match head.as_str() {
            "outcome" if self.peek() == Some(&CTok::Eq) => {
                self.pos += 1;
                let Some(CTok::Ident(x)) = self.bump().cloned() else {
                    return false;
                };
                x == status_name(self.outcome.status)
                    || self.outcome.preferred_label.as_deref() == Some(x.as_str())
            }
            "answer" if self.peek() == Some(&CTok::Eq) => {
                self.pos += 1;
                let Some(CTok::Ident(x)) = self.bump().cloned() else {
                    return false;
                };
                self.ctx.get("answer").and_then(|v| v.as_str()) == Some(x.as_str())
            }
            "kv" if self.peek() == Some(&CTok::LParen) => {
                self.pos += 1;
                let Some(CTok::Str(key)) = self.bump().cloned() else {
                    return false;
                };
                if self.peek() != Some(&CTok::RParen) {
                    return false;
                }
                self.pos += 1;
                if self.peek() != Some(&CTok::EqEq) {
                    return false;
                }
                self.pos += 1;
                let Some(CTok::Str(val)) = self.bump().cloned() else {
                    return false;
                };
                self.ctx.get(&key).and_then(|v| v.as_str()) == Some(val.as_str())
            }
            _ => false,
        }
    }
}
