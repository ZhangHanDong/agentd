use agentd_security::redaction::{ContentRedactor, RedactionError, RedactionLimits};

fn limits() -> RedactionLimits {
    RedactionLimits {
        max_input_bytes: 4_096,
        max_output_bytes: 4_096,
    }
}

#[test]
fn redaction_is_leftmost_longest_and_deterministic() {
    let redactor = ContentRedactor::compile(
        vec![b"token".to_vec(), b"token-extended".to_vec()],
        vec![r"api_[A-Za-z0-9]{8}".to_string()],
        limits(),
    )
    .expect("compile policy");

    let first = redactor
        .redact(b"token-extended token api_a1b2c3d4")
        .expect("redact");
    let second = redactor
        .redact(b"token-extended token api_a1b2c3d4")
        .expect("redact again");

    assert_eq!(first.bytes(), b"[REDACTED] [REDACTED] [REDACTED]");
    assert_eq!(first, second);
    assert_eq!(first.replacement_count(), 3);
}

#[test]
fn redaction_merges_overlapping_exact_and_pattern_matches() {
    let redactor =
        ContentRedactor::compile(vec![b"aba".to_vec()], vec!["xyx".to_string()], limits())
            .expect("compile overlapping policy");

    let output = redactor
        .redact(b"ababa xyxyx")
        .expect("redact overlapping occurrences");

    assert_eq!(output.bytes(), b"[REDACTED] [REDACTED]");
    assert_eq!(output.replacement_count(), 2);
}

#[test]
fn redaction_handles_binary_exact_values_and_utf8_policy_patterns() {
    let raw_secret = vec![0xff, 0x00, b's', b'e', b'c', b'r', b'e', b't'];
    let redactor = ContentRedactor::compile(
        vec![raw_secret.clone()],
        vec![r"password=[^\s]+".to_string()],
        limits(),
    )
    .expect("compile policy");
    let mut input = b"prefix ".to_vec();
    input.extend_from_slice(&raw_secret);
    input.extend_from_slice(" password=机密 suffix".as_bytes());

    let output = redactor.redact(&input).expect("redact binary content");
    assert_eq!(
        output.bytes(),
        "prefix [REDACTED] [REDACTED] suffix".as_bytes()
    );
    assert!(
        !output
            .bytes()
            .windows(raw_secret.len())
            .any(|part| part == raw_secret)
    );
}

#[test]
fn redaction_limits_fail_closed_without_exposing_input_or_rules() {
    let secret = b"do-not-print-this-secret".to_vec();
    let redactor = ContentRedactor::compile(
        vec![secret.clone()],
        vec!["credential=[a-z]+".to_string()],
        RedactionLimits {
            max_input_bytes: 8,
            max_output_bytes: 8,
        },
    )
    .expect("compile policy");

    let input_error = redactor
        .redact(&secret)
        .expect_err("oversized input must be rejected");
    assert_eq!(input_error, RedactionError::InputTooLarge);
    assert!(!format!("{input_error:?} {input_error}").contains("do-not-print"));
    assert!(!format!("{redactor:?}").contains("do-not-print"));

    let expansion = ContentRedactor::compile(vec![b"x".to_vec()], vec![], limits())
        .expect("compile expansion policy")
        .redact(&vec![b'x'; 500])
        .expect_err("replacement expansion must respect output bound");
    assert_eq!(expansion, RedactionError::OutputTooLarge);
}

#[test]
fn redaction_rejects_empty_values_empty_matches_and_invalid_patterns() {
    assert_eq!(
        ContentRedactor::compile(vec![Vec::new()], vec![], limits()).expect_err("empty exact"),
        RedactionError::EmptyExactValue
    );
    assert_eq!(
        ContentRedactor::compile(vec![], vec![".*".to_string()], limits())
            .expect_err("empty regex match"),
        RedactionError::PatternMatchesEmpty { index: 0 }
    );
    assert_eq!(
        ContentRedactor::compile(vec![], vec!["(".to_string()], limits())
            .expect_err("invalid regex"),
        RedactionError::InvalidPattern { index: 0 }
    );

    let boundary = ContentRedactor::compile(vec![], vec![r"\b".to_string()], limits())
        .expect("boundary does not match empty input");
    assert_eq!(
        boundary.redact(b"word").expect_err("zero-length match"),
        RedactionError::ZeroLengthMatch
    );
}
