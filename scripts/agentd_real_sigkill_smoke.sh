#!/usr/bin/env bash
# Real SIGKILL recovery smoke for the agentd daemon/store resume path.
# Default is non-destructive dry-run. Real execution requires:
#   AGENTD_REAL_SIGKILL_SMOKE=1 bash scripts/agentd_real_sigkill_smoke.sh --execute
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUN_ID="real-sigkill-smoke-$(date +%Y%m%d%H%M%S)"
PORT="18790"
STATE_DIR=""

usage() {
    cat <<'EOF'
usage: agentd_real_sigkill_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --run-id ID          Run id to pass to agentctl (default: timestamped)
  --port PORT          Local daemon port (default: 18790)
  --state-dir DIR      Evidence directory (default: .agentd/real-sigkill-smoke/<run-id>)
  -h, --help           Show this help

Real execution requires AGENTD_REAL_SIGKILL_SMOKE=1. This harness starts a local
daemon with a temporary wait.human workflow, kills only that daemon PID with
SIGKILL, restarts it on the same SQLite DB, then resumes through agentd mcp-stdio.
Dry-run and preflight-only never start or kill a daemon.
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
    STATE_DIR="$ROOT/.agentd/real-sigkill-smoke/$RUN_ID"
else
    STATE_DIR="$(abs_from_root "$STATE_DIR")"
fi

DAEMON_URL="http://127.0.0.1:$PORT"
HEALTH_URL="$DAEMON_URL/healthz"
AGENTD_BIN="$ROOT/target/debug/agentd"
AGENTCTL_BIN="$ROOT/target/debug/agentctl"
WORKFLOWS_DIR="$STATE_DIR/workflows"
WORKTREE_BASE="$STATE_DIR/worktrees"
DB_PATH="$STATE_DIR/agentd.db"
WORKFLOW_FILE="$WORKFLOWS_DIR/draft.dot"
PREFLIGHT_LOG="$STATE_DIR/preflight.log"
DAEMON_LOG="$STATE_DIR/daemon.log"
RESTARTED_DAEMON_LOG="$STATE_DIR/daemon.restarted.log"
AGENTCTL_OUT="$STATE_DIR/agentctl.out"
MCP_OUT="$STATE_DIR/mcp-submit-human-answer.out"
RUN_SNAPSHOT="$STATE_DIR/run_snapshot.json"
EVENTS_SNAPSHOT="$STATE_DIR/events.snapshot"
SUMMARY="$STATE_DIR/summary.txt"
DAEMON_PID=""

print_plan() {
    cat <<EOF
agentd real SIGKILL recovery smoke plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR
health: $HEALTH_URL

preflight:
  verify local tools: cargo, curl, sqlite3, agent-spec

build:
  cargo build -p agentd-bin -p agentctl

prepare:
  write temporary wait.human workflow: $WORKFLOW_FILE

daemon:
  $AGENTD_BIN --db-path '$DB_PATH' --port '$PORT' --workflows-dir '$WORKFLOWS_DIR' --repo-dir '$ROOT' --worktree-base '$WORKTREE_BASE'

trigger:
  $AGENTCTL_BIN run start --flow draft --workflows-dir '$WORKFLOWS_DIR' --daemon-url '$DAEMON_URL' '$RUN_ID'

kill/restart:
  kill -9 <daemon-pid-started-by-this-script>
  restart the same daemon command with the same --db-path

resume:
  $AGENTD_BIN --db-path '$DB_PATH' --workflows-dir '$WORKFLOWS_DIR' --repo-dir '$ROOT' --worktree-base '$WORKTREE_BASE' mcp-stdio
  tools/call submit_human_answer(wait_id, answer=approve)

evidence:
  workflows/draft.dot
  preflight.log
  daemon.log
  daemon.restarted.log
  agentctl.out
  mcp-submit-human-answer.out
  run_snapshot.json
  events.snapshot
  summary.txt

success criterion:
  after SIGKILL and restart, submitting the human answer through agentd mcp-stdio
  moves the run from parked to finished using the same SQLite database.
EOF
}

need_tool() {
    local tool="$1"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        return 1
    fi
}

preflight() {
    need_tool cargo
    need_tool curl
    need_tool sqlite3
    need_tool agent-spec
    echo "preflight ok"
}

write_workflow() {
    mkdir -p "$WORKFLOWS_DIR" "$WORKTREE_BASE"
    cat >"$WORKFLOW_FILE" <<'EOF'
digraph draft {
  "start" [shape=Mdiamond];
  "approve" [handler="wait.human", prompt="approve real SIGKILL recovery?"];
  "done" [shape=Msquare];

  "start" -> "approve";
  "approve" -> "done" [condition="answer=approve"];
}
EOF
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

start_daemon() {
    local log_path="$1"
    "$AGENTD_BIN" \
        --db-path "$DB_PATH" \
        --port "$PORT" \
        --workflows-dir "$WORKFLOWS_DIR" \
        --repo-dir "$ROOT" \
        --worktree-base "$WORKTREE_BASE" \
        >"$log_path" 2>&1 &
    DAEMON_PID="$!"
    wait_for_health
}

open_wait_id() {
    sqlite3 "$DB_PATH" \
        "SELECT id FROM human_waits WHERE answered_at IS NULL ORDER BY opened_at LIMIT 1;"
}

submit_human_answer() {
    local wait_id="$1"
    printf '{"jsonrpc":"2.0","id":"answer-1","method":"tools/call","params":{"name":"submit_human_answer","arguments":{"wait_id":"%s","answer":"approve","feedback":"real SIGKILL smoke"}}}\n' "$wait_id" |
        "$AGENTD_BIN" \
            --db-path "$DB_PATH" \
            --workflows-dir "$WORKFLOWS_DIR" \
            --repo-dir "$ROOT" \
            --worktree-base "$WORKTREE_BASE" \
            mcp-stdio >"$MCP_OUT"
}

capture_events() {
    curl -fsS --max-time 2 "$DAEMON_URL/runs/$RUN_ID/events?from_seq=0" >"$EVENTS_SNAPSHOT" 2>/dev/null || true
}

write_summary() {
    local result="$1"
    local wait_id="${2:-}"
    {
        echo "result: $result"
        echo "run_id: $RUN_ID"
        echo "wait_id: $wait_id"
        echo "daemon_url: $DAEMON_URL"
        echo "state_dir: $STATE_DIR"
        echo "db_path: $DB_PATH"
        echo "workflow: $WORKFLOW_FILE"
        echo "run_snapshot: $RUN_SNAPSHOT"
        echo "events_snapshot: $EVENTS_SNAPSHOT"
        echo "daemon_log: $DAEMON_LOG"
        echo "restarted_daemon_log: $RESTARTED_DAEMON_LOG"
        echo "agentctl_out: $AGENTCTL_OUT"
        echo "mcp_out: $MCP_OUT"
    } >"$SUMMARY"
}

run_execute() {
    if [[ "${AGENTD_REAL_SIGKILL_SMOKE:-}" != "1" ]]; then
        echo "refusing real SIGKILL smoke: set AGENTD_REAL_SIGKILL_SMOKE=1 with --execute" >&2
        return 2
    fi

    mkdir -p "$STATE_DIR"
    preflight | tee "$PREFLIGHT_LOG"
    write_workflow

    cargo build -p agentd-bin -p agentctl

    trap cleanup EXIT
    start_daemon "$DAEMON_LOG"

    "$AGENTCTL_BIN" run start \
        --flow draft \
        --workflows-dir "$WORKFLOWS_DIR" \
        --daemon-url "$DAEMON_URL" \
        "$RUN_ID" \
        >"$AGENTCTL_OUT" 2>&1

    local wait_id
    wait_id="$(open_wait_id)"
    if [[ -z "$wait_id" ]]; then
        write_summary "no_open_human_wait"
        echo "no open human wait found in $DB_PATH" >&2
        return 1
    fi

    kill -9 "$DAEMON_PID"
    wait "$DAEMON_PID" >/dev/null 2>&1 || true
    DAEMON_PID=""

    start_daemon "$RESTARTED_DAEMON_LOG"
    submit_human_answer "$wait_id"

    curl -fsS "$DAEMON_URL/runs/$RUN_ID" >"$RUN_SNAPSHOT"
    capture_events

    if grep -q '"status":"finished"' "$RUN_SNAPSHOT"; then
        write_summary "resumed_after_sigkill" "$wait_id"
        echo "real SIGKILL smoke finished; evidence: $STATE_DIR"
        return 0
    fi

    write_summary "resume_did_not_finish" "$wait_id"
    echo "run did not finish after SIGKILL resume; evidence: $STATE_DIR" >&2
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
