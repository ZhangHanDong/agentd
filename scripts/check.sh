#!/usr/bin/env bash
# Local mirror of the CI seven-layer gate.
# Exits non-zero on any failure. Intended to be runnable from a clean checkout.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> [1/7] cargo fmt --check"
cargo fmt --all --check

echo "==> [2/7] cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> [3/7] cargo nextest (workspace)"
if command -v cargo-nextest >/dev/null 2>&1; then
    # --no-tests=warn: an empty test set (early phases, or a crate with no
    # tests yet) must not fail the gate. nextest >=0.9.85 defaults to `fail`.
    cargo nextest run --workspace --no-tests=warn
else
    echo "    cargo-nextest not installed; falling back to cargo test"
    cargo test --workspace
fi

# True when specs/ exists and holds at least one .spec / .spec.md file.
have_specs() { [ -d specs ] && find specs -name '*.spec' -o -name '*.spec.md' 2>/dev/null | grep -q .; }

echo "==> [4/7] agent-spec lifecycle"
# `agent-spec lifecycle` takes ONE spec file (not a glob) and --format is
# text|json|md (NOT prompt-summary). Loop over every spec; run once per file
# and fail loudly (printing full output) on the first non-zero exit.
if command -v agent-spec >/dev/null 2>&1; then
    if have_specs; then
        while IFS= read -r spec; do
            if out=$(agent-spec lifecycle "$spec" --code . --min-score 0.7 --format text 2>&1); then
                echo "    -- $spec: $(echo "$out" | grep -Eo 'Pass rate: [0-9.]+%' | tail -1)"
            else
                echo "    -- $spec: FAILED"
                echo "$out"
                exit 1
            fi
        done < <(find specs -name '*.spec' -o -name '*.spec.md' | sort)
    else
        echo "    no specs/ yet; skipping (specs land in P0.0 Task 7+)"
    fi
else
    echo "    agent-spec not installed; skipping (CI will catch this)"
fi

echo "==> [5/7] agent-spec guard"
if command -v agent-spec >/dev/null 2>&1 && have_specs; then
    agent-spec guard --code .
else
    echo "    skipped (no agent-spec or no specs yet)"
fi

echo "==> [6/7] dot-validate (skipped until agentctl flow exists)"
# Will be wired up in P0.1 when agentctl gains the `flow` subcommand.

echo "==> [7/7] cross-deps-sanity"
# Forbidden references that would indicate boundary violations.
# IMPORTANT: scope to production source only (`crates/*/src/**`), not tests/ —
# test code legitimately constructs these literal strings as fixtures and
# regex patterns, and we don't want the gate to flag itself.
if rg --quiet --glob 'crates/*/src/**' 'palace\.db' . 2>/dev/null; then
    echo "ERROR: agentd must not reference palace.db (mempal's DB). See design §3.1."
    rg --line-number --glob 'crates/*/src/**' 'palace\.db' .
    exit 1
fi
if rg --quiet --glob 'crates/*/src/**' 'send-keys -l' . 2>/dev/null; then
    echo "ERROR: send-keys -l forbidden — use buffer path. See design §4.6."
    rg --line-number --glob 'crates/*/src/**' 'send-keys -l' .
    exit 1
fi

echo "✅ ready for PR"
