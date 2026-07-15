#!/usr/bin/env bash
# Real Matrix client bridge preflight smoke.
# Default is non-destructive dry-run. Real execution requires:
#   AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1 bash scripts/agentd_matrix_client_bridge_preflight_smoke.sh --execute --matrix-homeserver-url http://...
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUN_ID="matrix-preflight-smoke-$(date +%Y%m%d%H%M%S)"
STATE_DIR=""
AGENTD_BIN="$ROOT/target/debug/agentd"
CARGO_BIN="cargo"
SKIP_BUILD="0"
AGENTD_API="${AGENTD_MATRIX_AGENTD_API:-http://127.0.0.1:8787}"
MATRIX_HOMESERVER_URL="${AGENTD_MATRIX_HOMESERVER_URL:-}"
MATRIX_ACCESS_TOKEN="${AGENTD_MATRIX_ACCESS_TOKEN:-}"
MATRIX_USER_ID="${AGENTD_MATRIX_USER_ID:-}"
MATRIX_DEVICE_ID="${AGENTD_MATRIX_DEVICE_ID:-}"
ITERATIONS="1"

usage() {
    cat <<'EOF'
usage: agentd_matrix_client_bridge_preflight_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --run-id ID                    Smoke run id (default: timestamped)
  --state-dir DIR                Evidence directory
                                  (default: .agentd/matrix-preflight-smoke/<run-id>)
  --agentd-bin FILE              agentd binary to run (default: target/debug/agentd)
  --cargo-bin FILE               cargo binary to use for builds (default: cargo)
  --skip-build                   Do not run cargo build before execute
  --agentd-api URL               Local agentd API URL used only for config validation
                                  (default: AGENTD_MATRIX_AGENTD_API or http://127.0.0.1:8787)
  --matrix-homeserver-url URL    Matrix homeserver URL to preflight
                                  (or AGENTD_MATRIX_HOMESERVER_URL)
  --matrix-access-token TOKEN    Optional Matrix access token for whoami
                                  (or AGENTD_MATRIX_ACCESS_TOKEN)
  --matrix-user-id MXID          Optional Matrix user id for service config parity
                                  (or AGENTD_MATRIX_USER_ID)
  --matrix-device-id ID          Optional Matrix device id for service config parity
                                  (or AGENTD_MATRIX_DEVICE_ID)
  -h, --help                     Show this help

Real execution requires AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1. Dry-run and
preflight-only never start the daemon and never run Matrix preflight HTTP.
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
        --state-dir)
            STATE_DIR="${2:?missing --state-dir value}"
            shift 2
            ;;
        --agentd-bin)
            AGENTD_BIN="${2:?missing --agentd-bin value}"
            shift 2
            ;;
        --cargo-bin)
            CARGO_BIN="${2:?missing --cargo-bin value}"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD="1"
            shift
            ;;
        --agentd-api)
            AGENTD_API="${2:?missing --agentd-api value}"
            shift 2
            ;;
        --matrix-homeserver-url)
            MATRIX_HOMESERVER_URL="${2:?missing --matrix-homeserver-url value}"
            shift 2
            ;;
        --matrix-access-token)
            MATRIX_ACCESS_TOKEN="${2:?missing --matrix-access-token value}"
            shift 2
            ;;
        --matrix-user-id)
            MATRIX_USER_ID="${2:?missing --matrix-user-id value}"
            shift 2
            ;;
        --matrix-device-id)
            MATRIX_DEVICE_ID="${2:?missing --matrix-device-id value}"
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
    STATE_DIR="$ROOT/.agentd/matrix-preflight-smoke/$RUN_ID"
else
    STATE_DIR="$(abs_from_root "$STATE_DIR")"
fi
AGENTD_BIN="$(abs_from_root "$AGENTD_BIN")"

BRIDGE_STATE="$STATE_DIR/matrix-client-bridge-state.json"
PREFLIGHT_OUT="$STATE_DIR/preflight.out"
PREFLIGHT_ERR="$STATE_DIR/preflight.err"
SUMMARY="$STATE_DIR/summary.txt"

access_token_status() {
    # Stable redaction marker for evidence review: access_token: set (redacted)
    if [[ -n "$MATRIX_ACCESS_TOKEN" ]]; then
        printf 'set (redacted)\n'
    else
        printf 'not_set\n'
    fi
}

print_plan() {
    cat <<EOF
agentd Matrix client bridge preflight smoke plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR
agentd_bin: $AGENTD_BIN
agentd_api: $AGENTD_API
matrix_homeserver_url: ${MATRIX_HOMESERVER_URL:-<required>}
access_token: $(access_token_status)
matrix_user_id: ${MATRIX_USER_ID:-not_set}
matrix_device_id: ${MATRIX_DEVICE_ID:-not_set}

preflight:
  validate local inputs and optional agentd binary path
  real HTTP is only performed by --execute with AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1

build:
  $CARGO_BIN build -p agentd-bin

command:
  $AGENTD_BIN matrix-client-bridge-preflight --agentd-api '$AGENTD_API' --state '$BRIDGE_STATE' --iterations '$ITERATIONS' --matrix-homeserver-url '${MATRIX_HOMESERVER_URL:-<required>}'
  optional token/user/device flags are passed when set; token values are redacted from this plan

evidence:
  preflight.out
  preflight.err
  summary.txt

success criterion:
  agentd matrix-client-bridge-preflight reaches the configured homeserver,
  reports Matrix versions and optional whoami, and does not create the bridge
  cursor state file.
EOF
}

need_tool() {
    local tool="$1"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        return 1
    fi
}

validate_config() {
    if [[ -z "$MATRIX_HOMESERVER_URL" ]]; then
        echo "missing Matrix homeserver URL: pass --matrix-homeserver-url or set AGENTD_MATRIX_HOMESERVER_URL" >&2
        return 2
    fi

    if [[ "$SKIP_BUILD" != "1" ]]; then
        need_tool "$CARGO_BIN"
    fi

    if [[ "$SKIP_BUILD" == "1" && ! -x "$AGENTD_BIN" ]]; then
        echo "agentd binary is not executable: $AGENTD_BIN" >&2
        return 1
    fi
}

build_preflight_args() {
    AGENTD_PREFLIGHT_ARGS=(
        "matrix-client-bridge-preflight"
        "--agentd-api"
        "$AGENTD_API"
        "--state"
        "$BRIDGE_STATE"
        "--iterations"
        "$ITERATIONS"
        "--matrix-homeserver-url"
        "$MATRIX_HOMESERVER_URL"
    )
    if [[ -n "$MATRIX_ACCESS_TOKEN" ]]; then
        AGENTD_PREFLIGHT_ARGS+=("--matrix-access-token" "$MATRIX_ACCESS_TOKEN")
    fi
    if [[ -n "$MATRIX_USER_ID" ]]; then
        AGENTD_PREFLIGHT_ARGS+=("--matrix-user-id" "$MATRIX_USER_ID")
    fi
    if [[ -n "$MATRIX_DEVICE_ID" ]]; then
        AGENTD_PREFLIGHT_ARGS+=("--matrix-device-id" "$MATRIX_DEVICE_ID")
    fi
}

write_summary() {
    local result="$1"
    {
        echo "result: $result"
        echo "run_id: $RUN_ID"
        echo "state_dir: $STATE_DIR"
        echo "agentd_bin: $AGENTD_BIN"
        echo "agentd_api: $AGENTD_API"
        echo "matrix_homeserver_url: $MATRIX_HOMESERVER_URL"
        echo "access_token: $(access_token_status)"
        echo "matrix_user_id: ${MATRIX_USER_ID:-not_set}"
        echo "matrix_device_id: ${MATRIX_DEVICE_ID:-not_set}"
        echo "bridge_state: $BRIDGE_STATE"
        echo "preflight_out: $PREFLIGHT_OUT"
        echo "preflight_err: $PREFLIGHT_ERR"
    } >"$SUMMARY"
}

run_execute() {
    if [[ "${AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE:-}" != "1" ]]; then
        echo "refusing real Matrix preflight smoke: set AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1 with --execute" >&2
        return 2
    fi

    validate_config
    mkdir -p "$STATE_DIR"

    if [[ "$SKIP_BUILD" != "1" ]]; then
        "$CARGO_BIN" build -p agentd-bin
    fi

    build_preflight_args
    if "$AGENTD_BIN" "${AGENTD_PREFLIGHT_ARGS[@]}" >"$PREFLIGHT_OUT" 2>"$PREFLIGHT_ERR"; then
        if [[ -e "$BRIDGE_STATE" ]]; then
            write_summary "failed_state_mutation"
            echo "Matrix preflight unexpectedly created bridge cursor state: $BRIDGE_STATE" >&2
            return 1
        fi
        write_summary "finished"
        echo "matrix preflight smoke finished; evidence: $STATE_DIR"
        return 0
    fi

    write_summary "failed"
    echo "matrix preflight smoke failed; evidence: $STATE_DIR" >&2
    return 1
}

case "$MODE" in
    dry-run)
        print_plan
        ;;
    preflight-only)
        validate_config
        echo "preflight ok"
        ;;
    execute)
        run_execute
        ;;
    *)
        echo "internal error: unknown mode $MODE" >&2
        exit 2
        ;;
esac
