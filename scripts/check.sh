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

require_agent_spec() {
    if ! command -v agent-spec >/dev/null 2>&1; then
        echo "ERROR: agent-spec 1.0.0 is required" >&2
        exit 69
    fi
    version=$(agent-spec --version)
    if [ "$version" != "agent-spec 1.0.0" ]; then
        echo "ERROR: expected agent-spec 1.0.0, found $version" >&2
        exit 69
    fi
}

echo "==> [4/7] agent-spec verification"
# The shared verifier runs parse + lint for explicitly design-only contracts
# and the full lifecycle for every implementation contract.
if have_specs; then
    require_agent_spec
    while IFS= read -r spec; do
        if out=$(scripts/agentd_verify_spec.sh "$spec" 2>&1); then
            echo "    -- $spec: PASS"
        else
            echo "    -- $spec: FAILED"
            echo "$out"
            exit 1
        fi
    done < <(find specs -name '*.spec' -o -name '*.spec.md' | sort)
else
    echo "    no specs/ yet; skipping (specs land in P0.0 Task 7+)"
fi

echo "==> [5/7] changed-contract boundary guard"
if have_specs; then
    scripts/agentd_guard_changed_contract.sh --staged
else
    echo "    skipped (no specs yet)"
fi

echo "==> [6/7] dot-validate"
# Validate every shipped workflow with the real `agentctl flow validate`.
if ls workflows/*.dot >/dev/null 2>&1; then
    cargo build -q -p agentctl
    for f in workflows/*.dot; do
        echo "    -- $f"
        ./target/debug/agentctl flow validate "$f"
    done
else
    echo "    (no workflows/*.dot to validate)"
fi

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
