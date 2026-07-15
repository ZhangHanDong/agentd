#!/usr/bin/env bash
# Aggregate real-environment readiness checks without running real execute gates.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPTS_DIR="$ROOT/scripts"
MODE="dry-run"
RUN_ID="real-env-preflight-$(date +%Y%m%d%H%M%S)"
STATE_DIR=""

usage() {
    cat <<'EOF'
usage: agentd_real_env_preflight.sh [--dry-run|--preflight-only] [options]

Options:
  --run-id ID          Stable id used for nested preflight state paths
  --state-dir DIR      Nested preflight state root (default: .agentd/real-env-preflight/<run-id>)
  -h, --help           Show this help

This aggregate helper never runs AGENTD_REAL_* --execute gates. It only prints
the real-environment plan or runs existing --preflight-only checks.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)
            MODE="dry-run"
            shift
            ;;
        --preflight-only)
            MODE="preflight-only"
            shift
            ;;
        --execute)
            echo "agentd_real_env_preflight.sh does not run AGENTD_REAL_* --execute gates" >&2
            echo "run the individual real harness with its explicit opt-in when authorized" >&2
            exit 2
            ;;
        --run-id)
            RUN_ID="${2:?missing --run-id value}"
            shift 2
            ;;
        --state-dir)
            STATE_DIR="${2:?missing --state-dir value}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

abs_from_root() {
    local path="$1"
    if [[ "$path" = /* ]]; then
        printf '%s\n' "$path"
    else
        printf '%s/%s\n' "$ROOT" "$path"
    fi
}

if [[ -z "$STATE_DIR" ]]; then
    STATE_DIR="$ROOT/.agentd/real-env-preflight/$RUN_ID"
else
    STATE_DIR="$(abs_from_root "$STATE_DIR")"
fi

print_plan() {
    cat <<EOF
agentd real environment preflight plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR

This aggregate helper does not run AGENTD_REAL_* --execute gates.

preflight order:
  bash scripts/agentd_pr_history_status.sh HEAD main
  bash scripts/agentd_real_execute_smoke.sh --preflight-only
  bash scripts/agentd_real_sigkill_smoke.sh --preflight-only

failure behavior:
  the first failing preflight exits non-zero and stops the sequence before later
  component checks run.
EOF
}

run_step() {
    local label="$1"
    shift
    echo "[run] $label"
    "$@"
    echo "[ok] $label"
}

run_preflight() {
    echo "agentd real environment preflight"
    echo "repo: $ROOT"
    echo "run_id: $RUN_ID"
    echo "state_dir: $STATE_DIR"
    echo "no AGENTD_REAL_* --execute commands will run"

    (
        cd "$ROOT"
        run_step "git history" bash "$SCRIPTS_DIR/agentd_pr_history_status.sh" HEAD main
        run_step "real execute preflight" env \
            AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex \
            bash "$SCRIPTS_DIR/agentd_real_execute_smoke.sh" \
            --preflight-only \
            --run-id "$RUN_ID-execute" \
            --state-dir "$STATE_DIR/real-execute"
        run_step "real SIGKILL preflight" bash "$SCRIPTS_DIR/agentd_real_sigkill_smoke.sh" \
            --preflight-only \
            --run-id "$RUN_ID-sigkill" \
            --state-dir "$STATE_DIR/real-sigkill"
    )

    echo "real environment preflight ok"
    echo "no AGENTD_REAL_* --execute commands were run"
}

case "$MODE" in
    dry-run)
        print_plan
        ;;
    preflight-only)
        run_preflight
        ;;
    *)
        echo "internal error: unknown mode $MODE" >&2
        exit 2
        ;;
esac
