#!/usr/bin/env bash
# Real execute.dot smoke for the full agentd path.
# Default is non-destructive dry-run. Real execution requires:
#   AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUN_ID="real-execute-smoke-$(date +%Y%m%d%H%M%S)"
PORT="18789"
WAIT_SECONDS="600"
STATE_DIR=""
SPEC_FILE="$ROOT/.agentd/run/frozen.spec.md"

usage() {
    cat <<'EOF'
usage: agentd_real_execute_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --run-id ID          Run id to pass to agentctl (default: timestamped)
  --port PORT          Local daemon port (default: 18789)
  --state-dir DIR      Evidence directory (default: .agentd/real-execute-smoke/<run-id>)
  --spec-file FILE     Frozen spec to copy into .agentd/run/frozen.spec.md
  --wait-seconds N     Execute-mode wait for terminal run state (default: 600)
  -h, --help           Show this help

Real execution requires AGENTD_REAL_EXECUTE_SMOKE=1 and may use paid/authenticated
Claude Code plus GitHub PR creation. Dry-run and preflight-only never start the
daemon, tmux, Claude, or gh.
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
            MODE="execute"
            shift
            ;;
        --run-id)
            RUN_ID="${2:?missing --run-id value}"
            shift 2
            ;;
        --port)
            PORT="${2:?missing --port value}"
            shift 2
            ;;
        --state-dir)
            STATE_DIR="${2:?missing --state-dir value}"
            shift 2
            ;;
        --spec-file)
            SPEC_FILE="${2:?missing --spec-file value}"
            shift 2
            ;;
        --wait-seconds)
            WAIT_SECONDS="${2:?missing --wait-seconds value}"
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
    STATE_DIR="$ROOT/.agentd/real-execute-smoke/$RUN_ID"
else
    STATE_DIR="$(abs_from_root "$STATE_DIR")"
fi
SPEC_FILE="$(abs_from_root "$SPEC_FILE")"

BASE_REMOTE="origin"
BASE_BRANCH="main"
BASE_REF="$BASE_REMOTE/$BASE_BRANCH"
DAEMON_URL="http://127.0.0.1:$PORT"
HEALTH_URL="$DAEMON_URL/healthz"
AGENTD_BIN="$ROOT/target/debug/agentd"
AGENTCTL_BIN="$ROOT/target/debug/agentctl"
WORKFLOWS_DIR="$ROOT/workflows"
WORKTREE_BASE="$STATE_DIR/worktrees"
DB_PATH="$STATE_DIR/agentd.db"
RUNTIME_DIR="$ROOT/.agentd/run"
RUNTIME_SPEC="$RUNTIME_DIR/frozen.spec.md"
RUNTIME_PLAN="$RUNTIME_DIR/plan.md"
FROZEN_SPEC_COPY="$STATE_DIR/frozen.spec.md"
PLAN_COPY="$STATE_DIR/plan.md"
PREFLIGHT_LOG="$STATE_DIR/preflight.log"
DAEMON_LOG="$STATE_DIR/daemon.log"
AGENTCTL_OUT="$STATE_DIR/agentctl.out"
RUN_SNAPSHOT="$STATE_DIR/run_snapshot.json"
EVENTS_SNAPSHOT="$STATE_DIR/events.snapshot"
SUMMARY="$STATE_DIR/summary.txt"
DAEMON_PID=""

print_plan() {
    cat <<EOF
agentd real execute smoke plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR
spec_file: $SPEC_FILE
health: $HEALTH_URL

preflight:
  verify local tools, Claude MCP flag support, gh auth, and git history readiness
  compare HEAD with origin/main before starting daemon or agents
  bash scripts/agentd_pr_history_status.sh HEAD main

build:
  cargo build -p agentd-bin -p agentctl

prepare:
  cp '$SPEC_FILE' '$RUNTIME_SPEC'
  bash scripts/agentd_write_plan.sh '$RUNTIME_SPEC' '$RUNTIME_PLAN'

daemon:
  $AGENTD_BIN --db-path '$DB_PATH' --port '$PORT' --workflows-dir '$WORKFLOWS_DIR' --repo-dir '$ROOT' --worktree-base '$WORKTREE_BASE'

trigger:
  $AGENTCTL_BIN run start --flow execute --workflows-dir '$WORKFLOWS_DIR' --daemon-url '$DAEMON_URL' '$RUN_ID'

evidence:
  frozen.spec.md
  plan.md
  preflight.log
  daemon.log
  agentctl.out
  run_snapshot.json
  events.snapshot
  summary.txt

success criterion:
  execute.dot reaches a terminal run state after real agents implement/review,
  publish_branch pushes agentd/<task_run_id>, and open_pr opens a real PR.

failure evidence:
  if there is a captured preflight error from scripts/agentd_open_pr.sh, the
  harness records the failed run evidence and exits non-zero; that is
  failure evidence, not success.
EOF
}

need_tool() {
    local tool="$1"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        return 1
    fi
}

preflight_base_history() {
    (cd "$ROOT" && bash scripts/agentd_pr_history_status.sh HEAD "$BASE_BRANCH")
}

preflight() {
    need_tool cargo
    need_tool tmux
    need_tool claude
    need_tool agent-spec
    need_tool curl
    need_tool git
    need_tool gh

    if ! claude --help 2>&1 | grep -q -- "--mcp-config"; then
        echo "claude prerequisite failed: --mcp-config not present in claude --help" >&2
        return 1
    fi
    if ! gh auth status >/dev/null 2>&1; then
        echo "gh prerequisite failed: gh auth status did not succeed" >&2
        return 1
    fi
    preflight_base_history
    echo "preflight ok"
}

cleanup() {
    if [[ -n "${DAEMON_PID:-}" ]] && kill -0 "$DAEMON_PID" >/dev/null 2>&1; then
        kill "$DAEMON_PID" >/dev/null 2>&1 || true
        wait "$DAEMON_PID" >/dev/null 2>&1 || true
    fi
}

wait_for_health() {
    local deadline=$((SECONDS + 30))
    while (( SECONDS < deadline )); do
        if curl -fsS "$HEALTH_URL" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "daemon did not become healthy at $HEALTH_URL" >&2
    return 1
}

capture_events() {
    curl -fsS --max-time 2 "$DAEMON_URL/runs/$RUN_ID/events?from_seq=0" >"$EVENTS_SNAPSHOT" 2>/dev/null || true
}

canonical_file_path() {
    local path="$1"
    local dir
    dir="$(cd "$(dirname "$path")" && pwd -P)"
    printf '%s/%s\n' "$dir" "$(basename "$path")"
}

write_summary() {
    local result="$1"
    {
        echo "result: $result"
        echo "run_id: $RUN_ID"
        echo "daemon_url: $DAEMON_URL"
        echo "state_dir: $STATE_DIR"
        echo "spec_file: $SPEC_FILE"
        echo "frozen_spec: $FROZEN_SPEC_COPY"
        echo "plan: $PLAN_COPY"
        echo "run_snapshot: $RUN_SNAPSHOT"
        echo "events_snapshot: $EVENTS_SNAPSHOT"
        echo "daemon_log: $DAEMON_LOG"
        echo "agentctl_out: $AGENTCTL_OUT"
    } >"$SUMMARY"
}

prepare_runtime_spec_and_plan() {
    if [[ ! -f "$SPEC_FILE" ]]; then
        echo "frozen spec not found: $SPEC_FILE" >&2
        return 1
    fi
    mkdir -p "$STATE_DIR" "$RUNTIME_DIR"
    cp "$SPEC_FILE" "$FROZEN_SPEC_COPY"
    if [[ "$(canonical_file_path "$SPEC_FILE")" != "$(canonical_file_path "$RUNTIME_SPEC")" ]]; then
        cp "$SPEC_FILE" "$RUNTIME_SPEC"
    fi
    bash "$ROOT/scripts/agentd_write_plan.sh" "$RUNTIME_SPEC" "$RUNTIME_PLAN"
    cp "$RUNTIME_PLAN" "$PLAN_COPY"
}

run_execute() {
    if [[ "${AGENTD_REAL_EXECUTE_SMOKE:-}" != "1" ]]; then
        echo "refusing real execute smoke: set AGENTD_REAL_EXECUTE_SMOKE=1 with --execute" >&2
        return 2
    fi

    mkdir -p "$STATE_DIR"
    preflight | tee "$PREFLIGHT_LOG"
    prepare_runtime_spec_and_plan

    cargo build -p agentd-bin -p agentctl

    AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1 "$AGENTD_BIN" \
        --db-path "$DB_PATH" \
        --port "$PORT" \
        --workflows-dir "$WORKFLOWS_DIR" \
        --repo-dir "$ROOT" \
        --worktree-base "$WORKTREE_BASE" \
        >"$DAEMON_LOG" 2>&1 &
    DAEMON_PID="$!"
    trap cleanup EXIT

    wait_for_health

    "$AGENTCTL_BIN" run start \
        --flow execute \
        --workflows-dir "$WORKFLOWS_DIR" \
        --daemon-url "$DAEMON_URL" \
        "$RUN_ID" \
        >"$AGENTCTL_OUT" 2>&1

    local deadline=$((SECONDS + WAIT_SECONDS))
    local terminal="0"
    while (( SECONDS < deadline )); do
        if curl -fsS "$DAEMON_URL/runs/$RUN_ID" >"$RUN_SNAPSHOT.tmp"; then
            mv "$RUN_SNAPSHOT.tmp" "$RUN_SNAPSHOT"
            if grep -q '"status":"finished"\|"status":"failed"' "$RUN_SNAPSHOT"; then
                terminal="1"
                break
            fi
        fi
        sleep 2
    done
    rm -f "$RUN_SNAPSHOT.tmp"
    capture_events

    if [[ "$terminal" != "1" ]]; then
        write_summary "timeout_waiting_for_execute_terminal_state"
        echo "timed out waiting for run $RUN_ID to reach a terminal state" >&2
        return 1
    fi

    if grep -q '"status":"finished"' "$RUN_SNAPSHOT"; then
        write_summary "finished"
        echo "real execute smoke finished; evidence: $STATE_DIR"
        return 0
    fi

    write_summary "failed"
    echo "real execute smoke reached failed terminal state; evidence: $STATE_DIR" >&2
    return 1
}

case "$MODE" in
    dry-run)
        print_plan
        ;;
    preflight-only)
        preflight
        ;;
    execute)
        run_execute
        ;;
    *)
        echo "internal error: unknown mode $MODE" >&2
        exit 2
        ;;
esac
