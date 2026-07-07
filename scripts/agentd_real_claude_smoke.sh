#!/usr/bin/env bash
# Real Claude Code smoke for the agentd stdio MCP path.
# Default is non-destructive dry-run. Real execution requires:
#   AGENTD_REAL_CLAUDE_SMOKE=1 bash scripts/agentd_real_claude_smoke.sh --execute
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUN_ID="real-claude-smoke-$(date +%Y%m%d%H%M%S)"
PORT="18787"
WAIT_SECONDS="300"
STATE_DIR=""

usage() {
    cat <<'EOF'
usage: agentd_real_claude_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --run-id ID          Run id to pass to agentctl (default: timestamped)
  --port PORT          Local daemon port (default: 18787)
  --state-dir DIR      Evidence directory (default: .agentd/real-claude-smoke/<run-id>)
  --wait-seconds N     Execute-mode wait for real Claude to advance run (default: 300)
  -h, --help           Show this help

Real execution requires AGENTD_REAL_CLAUDE_SMOKE=1 and may use paid/authenticated
Claude Code. Dry-run and preflight-only never start the daemon or tmux.
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

if [[ -z "$STATE_DIR" ]]; then
    STATE_DIR="$ROOT/.agentd/real-claude-smoke/$RUN_ID"
fi

DAEMON_URL="http://127.0.0.1:$PORT"
HEALTH_URL="$DAEMON_URL/healthz"
AGENTD_BIN="$ROOT/target/debug/agentd"
AGENTCTL_BIN="$ROOT/target/debug/agentctl"
WORKFLOWS_DIR="$ROOT/workflows"
WORKTREE_BASE="$STATE_DIR/worktrees"
DB_PATH="$STATE_DIR/agentd.db"
ISSUE_FILE="$STATE_DIR/issue.md"
PREFLIGHT_LOG="$STATE_DIR/preflight.log"
DAEMON_LOG="$STATE_DIR/daemon.log"
AGENTCTL_OUT="$STATE_DIR/agentctl.out"
RUN_SNAPSHOT="$STATE_DIR/run_snapshot.json"
EVENTS_SNAPSHOT="$STATE_DIR/events.snapshot"
SUMMARY="$STATE_DIR/summary.txt"
RUNTIME_ISSUE="$ROOT/.agentd/run/issue.md"
TMUX_SESSION="agentd-spec-writer"
DAEMON_PID=""

print_plan() {
    cat <<EOF
agentd real Claude stdio smoke plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR
health: $HEALTH_URL

build:
  cargo build -p agentd-bin -p agentctl

daemon:
  $AGENTD_BIN --db-path '$DB_PATH' --port '$PORT' --workflows-dir '$WORKFLOWS_DIR' --repo-dir '$ROOT' --worktree-base '$WORKTREE_BASE'

trigger:
  $AGENTCTL_BIN run start --flow draft --workflows-dir '$WORKFLOWS_DIR' --daemon-url '$DAEMON_URL' '$RUN_ID'

evidence:
  issue.md
  preflight.log
  daemon.log
  agentctl.out
  run_snapshot.json
  events.snapshot
  summary.txt

success criterion:
  a real authenticated Claude Code process in tmux session '$TMUX_SESSION' calls submit_outcome over the agentd MCP server, and GET /runs/$RUN_ID no longer reports current_node=propose_spec.
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
    need_tool tmux
    need_tool claude
    need_tool agent-spec
    need_tool curl

    if ! claude --help 2>&1 | grep -q -- "--mcp-config"; then
        echo "claude prerequisite failed: --mcp-config not present in claude --help" >&2
        return 1
    fi
    echo "preflight ok"
}

write_issue() {
    mkdir -p "$STATE_DIR" "$ROOT/.agentd/run"
    cat >"$ISSUE_FILE" <<EOF
# agentd real Claude stdio smoke

This issue exists to prove that a real authenticated Claude Code process can use
the agentd MCP server named "agentd" and call submit_outcome for run "$RUN_ID".

Expected operator evidence: the run leaves current_node=propose_spec after the
agent submits an outcome through tools/call submit_outcome.
EOF
    cp "$ISSUE_FILE" "$RUNTIME_ISSUE"
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

write_summary() {
    local result="$1"
    {
        echo "result: $result"
        echo "run_id: $RUN_ID"
        echo "daemon_url: $DAEMON_URL"
        echo "tmux_session: $TMUX_SESSION"
        echo "state_dir: $STATE_DIR"
        echo "run_snapshot: $RUN_SNAPSHOT"
        echo "events_snapshot: $EVENTS_SNAPSHOT"
        echo "daemon_log: $DAEMON_LOG"
        echo "agentctl_out: $AGENTCTL_OUT"
    } >"$SUMMARY"
}

run_execute() {
    if [[ "${AGENTD_REAL_CLAUDE_SMOKE:-}" != "1" ]]; then
        echo "refusing real Claude smoke: set AGENTD_REAL_CLAUDE_SMOKE=1 with --execute" >&2
        return 2
    fi

    mkdir -p "$STATE_DIR"
    preflight | tee "$PREFLIGHT_LOG"
    write_issue

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
        --flow draft \
        --workflows-dir "$WORKFLOWS_DIR" \
        --daemon-url "$DAEMON_URL" \
        "$RUN_ID" \
        >"$AGENTCTL_OUT" 2>&1

    local deadline=$((SECONDS + WAIT_SECONDS))
    local advanced="0"
    while (( SECONDS < deadline )); do
        if curl -fsS "$DAEMON_URL/runs/$RUN_ID" >"$RUN_SNAPSHOT.tmp"; then
            mv "$RUN_SNAPSHOT.tmp" "$RUN_SNAPSHOT"
            if grep -q '"status":"finished"\|"status":"failed"' "$RUN_SNAPSHOT"; then
                advanced="1"
                break
            fi
            if ! grep -q '"current_node":"propose_spec"' "$RUN_SNAPSHOT"; then
                advanced="1"
                break
            fi
        fi
        sleep 2
    done
    rm -f "$RUN_SNAPSHOT.tmp"
    capture_events

    if [[ "$advanced" != "1" ]]; then
        write_summary "timeout_waiting_for_real_claude_submit_outcome"
        echo "timed out waiting for run $RUN_ID to advance beyond propose_spec" >&2
        echo "inspect tmux session: $TMUX_SESSION" >&2
        return 1
    fi

    write_summary "advanced"
    echo "real Claude smoke advanced; evidence: $STATE_DIR"
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
