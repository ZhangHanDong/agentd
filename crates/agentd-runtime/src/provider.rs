//! Provider-specific command construction and native session reference capture.

use std::collections::BTreeMap;
use std::path::PathBuf;

use agentd_core::ports::{NativeRuntimeError, RuntimeCommand, RuntimeProvider};
use serde_json::Value;

const MAX_NATIVE_REF_BYTES: usize = 512;
const MAX_INSPECTION_BYTES: usize = 256 * 1024;

/// Provider command configuration kept outside durable runtime identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCommand {
    pub provider: RuntimeProvider,
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub custom_resume_arguments: Option<Vec<String>>,
}

/// Stateless adapter for supported interactive agent CLIs.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeProviderAdapter;

impl RuntimeProviderAdapter {
    /// Build an initial or provider-native resume command.
    pub fn command(
        configuration: &ProviderCommand,
        native_session_ref: Option<&str>,
    ) -> Result<RuntimeCommand, NativeRuntimeError> {
        validate_configuration(configuration)?;
        let mut arguments = configuration.arguments.clone();
        if let Some(reference) = native_session_ref {
            validate_native_ref(reference)?;
            match configuration.provider {
                RuntimeProvider::Codex => {
                    arguments.push("resume".to_string());
                    arguments.push(reference.to_string());
                }
                RuntimeProvider::ClaudeCode => {
                    arguments.push("--resume".to_string());
                    arguments.push(reference.to_string());
                }
                RuntimeProvider::Custom => {
                    let template =
                        configuration
                            .custom_resume_arguments
                            .as_ref()
                            .ok_or_else(|| {
                                NativeRuntimeError::Invalid(
                                    "custom provider does not define native resume arguments"
                                        .to_string(),
                                )
                            })?;
                    arguments.extend(
                        template
                            .iter()
                            .map(|argument| argument.replace("{native_session_ref}", reference)),
                    );
                }
            }
        }
        Ok(RuntimeCommand {
            program: configuration.program.clone(),
            arguments,
            environment: configuration.environment.clone(),
            working_directory: configuration.working_directory.clone(),
        })
    }

    /// Extract a bounded provider-native session reference from redacted output.
    #[must_use]
    pub fn extract_native_session_ref(
        provider: RuntimeProvider,
        redacted_output: &[u8],
    ) -> Option<String> {
        if redacted_output.is_empty() || redacted_output.len() > MAX_INSPECTION_BYTES {
            return None;
        }
        let text = std::str::from_utf8(redacted_output).ok()?;
        json_native_ref(text, provider).or_else(|| text_native_ref(text, provider))
    }
}

fn validate_configuration(configuration: &ProviderCommand) -> Result<(), NativeRuntimeError> {
    if configuration.program.trim().is_empty()
        || configuration.program.contains('\0')
        || configuration
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
        || configuration.environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains('\0')
                || value.contains('\0')
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(NativeRuntimeError::Invalid(
            "provider command contains an invalid program, argument, or environment entry"
                .to_string(),
        ));
    }
    if configuration.provider != RuntimeProvider::Custom
        && configuration.custom_resume_arguments.is_some()
    {
        return Err(NativeRuntimeError::Invalid(
            "custom resume arguments are only valid for custom providers".to_string(),
        ));
    }
    Ok(())
}

fn validate_native_ref(reference: &str) -> Result<(), NativeRuntimeError> {
    if valid_native_ref(reference) {
        Ok(())
    } else {
        Err(NativeRuntimeError::Invalid(
            "provider native session reference is invalid".to_string(),
        ))
    }
}

fn valid_native_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= MAX_NATIVE_REF_BYTES
        && reference == reference.trim()
        && reference
            .chars()
            .all(|character| !character.is_control() && !character.is_whitespace())
}

fn json_native_ref(text: &str, provider: RuntimeProvider) -> Option<String> {
    let keys = match provider {
        RuntimeProvider::Codex => &["thread_id", "session_id", "conversation_id"][..],
        RuntimeProvider::ClaudeCode => &["session_id", "conversation_id"][..],
        RuntimeProvider::Custom => &["session_id", "native_session_ref"][..],
    };
    text.lines().find_map(|line| {
        let value = serde_json::from_str::<Value>(line.trim()).ok()?;
        find_json_string(&value, keys).filter(|reference| valid_native_ref(reference))
    })
}

fn find_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(reference) = map.get(*key).and_then(Value::as_str) {
                    return Some(reference.to_string());
                }
            }
            map.values().find_map(|value| find_json_string(value, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_json_string(value, keys)),
        _ => None,
    }
}

fn text_native_ref(text: &str, provider: RuntimeProvider) -> Option<String> {
    let markers = match provider {
        RuntimeProvider::Codex => &["thread_id=", "thread id:", "session_id=", "session id:"][..],
        RuntimeProvider::ClaudeCode => &["session_id=", "session id:"][..],
        RuntimeProvider::Custom => &["native_session_ref=", "session_id="][..],
    };
    text.lines().find_map(|line| {
        let lowercase = line.to_ascii_lowercase();
        markers.iter().find_map(|marker| {
            let offset = lowercase.find(marker)? + marker.len();
            let candidate = line[offset..]
                .trim()
                .trim_matches(|character: char| matches!(character, '"' | '\'' | ',' | ';'))
                .split_whitespace()
                .next()?;
            valid_native_ref(candidate).then(|| candidate.to_string())
        })
    })
}
