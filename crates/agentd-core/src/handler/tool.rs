//! The `tool` handler. Shells out via the `CommandRunner` port and maps the
//! result to an `Outcome`. Synchronous — never parks. agentd-core writes no
//! file here: when `artifact_path=` is set it records an `Artifact` *pointer*
//! (kind + path + sha256 + byte length of captured stdout) per design §3.1.

use std::collections::HashMap;
use std::time::Duration;

use crate::CoreError;
use crate::engine::HandlerStep;
use crate::handler::{Handler, HandlerCtx, sha256_hex};
use crate::ports::RunOpts;
use crate::types::{Artifact, ArtifactKind, Outcome, Status};

const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug)]
pub struct ToolHandler;

#[async_trait::async_trait]
impl Handler for ToolHandler {
    async fn run(&self, ctx: &mut HandlerCtx<'_>) -> Result<HandlerStep, CoreError> {
        // Parse `cmd` into program + args before any await so its borrow of
        // `ctx` ends before we touch `ctx.ports`.
        let (program, args) = {
            let node_id = ctx.node.id.clone();
            let cmd = ctx.node_attr("cmd").ok_or_else(|| {
                CoreError::Invariant(format!("tool node '{node_id}' missing cmd attribute"))
            })?;
            // Variables for `${...}` substitution (R2): every string value in the
            // run context, including `worktree` when codergen staged an allocated
            // task-run path. Substitution runs PER ARGV ELEMENT (after the
            // whitespace split), so a value with spaces stays one arg.
            let vars: HashMap<String, String> = ctx
                .context
                .0
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
            let mut parts = cmd.split_whitespace();
            let raw_program = parts.next().ok_or_else(|| {
                CoreError::Invariant(format!("tool node '{node_id}' has empty cmd"))
            })?;
            let program = substitute(raw_program, &node_id, &vars)?;
            let args: Vec<String> = parts
                .map(|p| substitute(p, &node_id, &vars))
                .collect::<Result<_, _>>()?;
            (program, args)
        };
        let timeout = ctx
            .node_attr("timeout_secs")
            .and_then(|s| s.parse::<u64>().ok())
            .map_or_else(
                || Duration::from_secs(DEFAULT_TIMEOUT_SECS),
                Duration::from_secs,
            );
        let artifact_path = ctx.node_attr("artifact_path").map(ToString::to_string);
        let static_context_updates = ctx
            .node_attr("context_updates")
            .map(|raw| parse_static_context_updates(raw, &ctx.node.id))
            .transpose()?
            .unwrap_or_default();

        let opts = RunOpts {
            timeout,
            // Tool nodes run in the DAEMON cwd, not a worktree (design-faithful C1
            // redirect): a code tool instead receives the worktree as an explicit
            // `--code <worktree>` argument via variable substitution (restored in
            // R2), so cwd stays where the `.agentd/run/` runtime-state convention
            // lives. Threading the worktree to cwd here (C1a) broke the tools that
            // read from `.agentd/run/` (untracked, so absent from a git worktree).
            ..RunOpts::default()
        };
        match ctx.ports.runner.run(&program, &args, opts).await {
            // The command ran. A clean exit is a deterministic outcome:
            // 0 → Success, anything else → Fail.
            Ok(output) if output.status == 0 => {
                let mut outcome = Outcome::success();
                if let Some(path) = artifact_path {
                    let bytes = output.stdout.as_bytes();
                    outcome.artifacts.push(Artifact {
                        kind: ArtifactKind::Transcript,
                        path: std::path::PathBuf::from(path),
                        sha256: sha256_hex(bytes),
                        bytes: bytes.len() as u64,
                    });
                }
                for (key, value) in static_context_updates {
                    ctx.stage(key, serde_json::Value::String(value));
                }
                Ok(HandlerStep::Done(outcome))
            }
            Ok(_) => Ok(HandlerStep::Done(Outcome::fail())),
            // The command could not run / timed out / was killed — transient, so
            // Retry (the engine's D8c retry bound caps how often this re-runs).
            Err(_e) => Ok(HandlerStep::Done(Outcome {
                status: Status::Retry,
                ..Outcome::success()
            })),
        }
    }
}

/// Replace every `${name}` in `s` with `vars[name]`. An unknown variable or an
/// unterminated `${` is a LOUD error (`CoreError::Invariant`) — never a silent
/// passthrough. Single pass: a substituted value that itself contains `${...}` is
/// NOT re-expanded. Applied per argv element (after the whitespace split), so a
/// value with spaces stays one argument.
fn substitute(s: &str, node_id: &str, vars: &HashMap<String, String>) -> Result<String, CoreError> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let end = after.find('}').ok_or_else(|| {
            CoreError::Invariant(format!(
                "tool node '{node_id}' cmd has an unterminated '${{' in {s:?}"
            ))
        })?;
        let name = &after[..end];
        let val = vars.get(name).ok_or_else(|| {
            CoreError::Invariant(format!(
                "tool node '{node_id}' cmd references undefined variable '${{{name}}}'"
            ))
        })?;
        out.push_str(val);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn parse_static_context_updates(
    raw: &str,
    node_id: &str,
) -> Result<Vec<(String, String)>, CoreError> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key, value) = entry.split_once('=').ok_or_else(|| {
                CoreError::Invariant(format!(
                    "tool node '{node_id}' context_updates entry '{entry}' must use key=value"
                ))
            })?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                return Err(CoreError::Invariant(format!(
                    "tool node '{node_id}' context_updates entry '{entry}' has empty key or value"
                )));
            }
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_static_context_updates, substitute};
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn substitute_replaces_known_vars() {
        let out = substitute("x ${a} y ${b}", "n", &vars(&[("a", "1"), ("b", "2")]))
            .expect("known vars substitute");
        assert_eq!(out, "x 1 y 2");
    }

    #[test]
    fn substitute_unknown_var_is_error() {
        let err = substitute("${nope}", "n", &vars(&[])).expect_err("unknown var is an error");
        assert!(
            format!("{err:?}").contains("nope"),
            "the error names the undefined var, not a silent passthrough: {err:?}"
        );
    }

    #[test]
    fn substitute_leaves_plain_text_unchanged() {
        let input = "cat .agentd/run/frozen.spec.md";
        let out = substitute(input, "n", &vars(&[("a", "1")])).expect("no-token passes through");
        assert_eq!(out, input, "text without `${{` is byte-identical");
    }

    #[test]
    fn static_context_updates_parse_key_value_pairs() {
        let updates = parse_static_context_updates("a=one, b = two", "n").expect("parse");
        assert_eq!(
            updates,
            vec![
                ("a".to_string(), "one".to_string()),
                ("b".to_string(), "two".to_string())
            ]
        );
    }

    #[test]
    fn static_context_updates_reject_malformed_entries() {
        let err = parse_static_context_updates("no_equals", "n").expect_err("malformed");
        assert!(
            format!("{err:?}").contains("key=value"),
            "error explains the expected syntax: {err:?}"
        );
    }
}
