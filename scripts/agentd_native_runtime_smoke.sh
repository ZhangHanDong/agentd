#!/usr/bin/env bash
# Real Codex smoke for the native PTY runtime. This never selects Claude Code.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${AGENTD_REAL_NATIVE_RUNTIME_SMOKE:-}" != "1" ]]; then
    echo "refusing real execution: set AGENTD_REAL_NATIVE_RUNTIME_SMOKE=1" >&2
    exit 2
fi

if [[ "${AGENTD_NATIVE_RUNTIME_SMOKE_PROVIDER:-codex}" != "codex" ]]; then
    echo "unsupported smoke provider: only codex is allowed" >&2
    exit 2
fi

if ! command -v "${AGENTD_CODEX_BIN:-codex}" >/dev/null 2>&1; then
    echo "codex CLI is not available" >&2
    exit 2
fi

cd "$ROOT"
exec cargo test -p agentd-runtime --test real_codex_native_runtime \
    real_codex_runs_through_native_pty_and_archives_transcript -- --exact --nocapture
