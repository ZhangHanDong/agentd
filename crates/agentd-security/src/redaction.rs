//! Bounded byte-oriented content redaction for transcripts, logs, and audit payloads.

use std::collections::BTreeSet;
use std::fmt;

use regex_automata::{Input, meta::Regex};
use thiserror::Error;

use agentd_core::ports::{ContentRedactionPort, SecurityError};

const REPLACEMENT: &[u8] = b"[REDACTED]";
const MAX_EXACT_RULES: usize = 1_024;
const MAX_PATTERN_RULES: usize = 128;
const MAX_PATTERN_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactionLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RedactionError {
    #[error("redaction limits are invalid")]
    InvalidLimits,
    #[error("redaction exact value is empty")]
    EmptyExactValue,
    #[error("redaction rule count exceeds the supported bound")]
    TooManyRules,
    #[error("redaction pattern at index {index} is invalid")]
    InvalidPattern { index: usize },
    #[error("redaction pattern at index {index} matches empty input")]
    PatternMatchesEmpty { index: usize },
    #[error("redaction input exceeds the configured bound")]
    InputTooLarge,
    #[error("redaction output exceeds the configured bound")]
    OutputTooLarge,
    #[error("redaction policy produced a zero-length match")]
    ZeroLengthMatch,
}

pub struct ContentRedactor {
    exact_values: Vec<Vec<u8>>,
    patterns: Vec<Regex>,
    limits: RedactionLimits,
}

impl fmt::Debug for ContentRedactor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContentRedactor")
            .field("exact_value_count", &self.exact_values.len())
            .field("pattern_count", &self.patterns.len())
            .field("limits", &self.limits)
            .finish()
    }
}

impl ContentRedactor {
    pub fn compile(
        exact_values: Vec<Vec<u8>>,
        policy_patterns: Vec<String>,
        limits: RedactionLimits,
    ) -> Result<Self, RedactionError> {
        if limits.max_input_bytes == 0 || limits.max_output_bytes == 0 {
            return Err(RedactionError::InvalidLimits);
        }
        if exact_values.len() > MAX_EXACT_RULES || policy_patterns.len() > MAX_PATTERN_RULES {
            return Err(RedactionError::TooManyRules);
        }
        if exact_values.iter().any(Vec::is_empty) {
            return Err(RedactionError::EmptyExactValue);
        }

        let mut exact_values = exact_values
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        exact_values
            .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));

        let mut patterns = Vec::with_capacity(policy_patterns.len());
        for (index, pattern) in policy_patterns.into_iter().enumerate() {
            if pattern.len() > MAX_PATTERN_BYTES {
                return Err(RedactionError::InvalidPattern { index });
            }
            let regex =
                Regex::new(&pattern).map_err(|_| RedactionError::InvalidPattern { index })?;
            if regex.is_match(b"") {
                return Err(RedactionError::PatternMatchesEmpty { index });
            }
            patterns.push(regex);
        }

        Ok(Self {
            exact_values,
            patterns,
            limits,
        })
    }

    pub fn redact(&self, input: &[u8]) -> Result<RedactedContent, RedactionError> {
        if input.len() > self.limits.max_input_bytes {
            return Err(RedactionError::InputTooLarge);
        }

        let mut best_end_by_start = vec![None::<usize>; input.len().saturating_add(1)];
        for start in 0..input.len() {
            if let Some(value) = self
                .exact_values
                .iter()
                .find(|value| input[start..].starts_with(value))
            {
                best_end_by_start[start] = Some(start + value.len());
            }
        }
        for pattern in &self.patterns {
            let mut search_start = 0;
            while search_start < input.len() {
                let Some(matched) = pattern.find(Input::new(input).span(search_start..input.len()))
                else {
                    break;
                };
                if matched.is_empty() {
                    return Err(RedactionError::ZeroLengthMatch);
                }
                let best = &mut best_end_by_start[matched.start()];
                *best = Some(best.map_or(matched.end(), |end| end.max(matched.end())));
                search_start = matched.start() + 1;
            }
        }

        let mut output = Vec::with_capacity(input.len().min(self.limits.max_output_bytes));
        let mut replacement_count = 0;
        let mut cursor = 0;
        while cursor < input.len() {
            if let Some(end) = best_end_by_start[cursor] {
                let mut end = end;
                let mut overlap_start = cursor + 1;
                while overlap_start < end {
                    if let Some(overlap_end) = best_end_by_start[overlap_start] {
                        end = end.max(overlap_end);
                    }
                    overlap_start += 1;
                }
                append_bounded(&mut output, REPLACEMENT, self.limits.max_output_bytes)?;
                replacement_count += 1;
                cursor = end;
                continue;
            }
            let next_match = best_end_by_start[cursor + 1..]
                .iter()
                .position(Option::is_some)
                .map_or(input.len(), |offset| cursor + 1 + offset);
            append_bounded(
                &mut output,
                &input[cursor..next_match],
                self.limits.max_output_bytes,
            )?;
            cursor = next_match;
        }

        Ok(RedactedContent {
            bytes: output,
            replacement_count,
        })
    }
}

#[async_trait::async_trait]
impl ContentRedactionPort for ContentRedactor {
    async fn redact_content(&self, content: &[u8]) -> Result<Vec<u8>, SecurityError> {
        self.redact(content)
            .map(RedactedContent::into_bytes)
            .map_err(|_| {
                SecurityError::Unavailable("required content redaction failed".to_string())
            })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RedactedContent {
    bytes: Vec<u8>,
    replacement_count: usize,
}

impl RedactedContent {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn replacement_count(&self) -> usize {
        self.replacement_count
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for RedactedContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactedContent")
            .field("byte_count", &self.bytes.len())
            .field("replacement_count", &self.replacement_count)
            .finish()
    }
}

fn append_bounded(
    output: &mut Vec<u8>,
    bytes: &[u8],
    max_output_bytes: usize,
) -> Result<(), RedactionError> {
    if output
        .len()
        .checked_add(bytes.len())
        .is_none_or(|length| length > max_output_bytes)
    {
        return Err(RedactionError::OutputTooLarge);
    }
    output.extend_from_slice(bytes);
    Ok(())
}
