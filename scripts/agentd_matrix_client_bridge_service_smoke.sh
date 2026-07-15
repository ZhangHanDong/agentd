#!/usr/bin/env bash
# Real Matrix client bridge service smoke.
# Default is non-destructive dry-run. Real execution requires:
#   AGENTD_REAL_MATRIX_SERVICE_SMOKE=1 bash scripts/agentd_matrix_client_bridge_service_smoke.sh --execute --matrix-homeserver-url http://...
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUN_ID="matrix-service-smoke-$(date +%Y%m%d%H%M%S)"
STATE_DIR=""
AGENTD_BIN="$ROOT/target/debug/agentd"
CARGO_BIN="cargo"
SKIP_BUILD="0"
AGENTD_API="${AGENTD_MATRIX_AGENTD_API:-http://127.0.0.1:8787}"
MATRIX_HOMESERVER_URL="${AGENTD_MATRIX_HOMESERVER_URL:-}"
MATRIX_USERNAME="${AGENTD_MATRIX_USERNAME:-}"
MATRIX_PASSWORD="${AGENTD_MATRIX_PASSWORD:-}"
MATRIX_USER_ID="${AGENTD_MATRIX_USER_ID:-}"
MATRIX_DEVICE_ID="${AGENTD_MATRIX_DEVICE_ID:-}"
MATRIX_ACCESS_TOKEN="${AGENTD_MATRIX_ACCESS_TOKEN:-}"
MATRIX_SYNC_TIMEOUT_MS="${AGENTD_MATRIX_SYNC_TIMEOUT_MS:-0}"
MATRIX_SDK_STORE="${AGENTD_MATRIX_SDK_STORE:-}"
MATRIX_BOT_USER_ID="${AGENTD_MATRIX_BOT_USER_ID:-}"
MATRIX_SERVER_NAME="${AGENTD_MATRIX_SERVER_NAME:-}"
MATRIX_AGENT_PREFIX="${AGENTD_MATRIX_AGENT_PREFIX:-ac_}"
MATRIX_TRUST_MODE="${AGENTD_MATRIX_TRUST_MODE:-audit}"
MATRIX_PUPPET_STATE="${AGENTD_MATRIX_PUPPET_STATE:-}"
MATRIX_AGENT_PASSWORD_SECRET="${AGENTD_MATRIX_AGENT_PASSWORD_SECRET:-}"
MATRIX_AGENT_PASSWORD_TEMPLATE="${AGENTD_MATRIX_AGENT_PASSWORD_TEMPLATE:-}"
MATRIX_ALLOW_LEGACY_AGENT_PASSWORD="${AGENTD_MATRIX_ALLOW_LEGACY_AGENT_PASSWORD:-0}"
MATRIX_REGISTRATION_TOKEN="${AGENTD_MATRIX_REGISTRATION_TOKEN:-}"
ITERATIONS="${AGENTD_MATRIX_SERVICE_ITERATIONS:-1}"
MATRIX_AGENTS=()
MATRIX_SKIP_AGENTS=()
MATRIX_TRUSTED_INVITERS=()
MATRIX_IGNORED_SENDERS=()

usage() {
    cat <<'EOF'
usage: agentd_matrix_client_bridge_service_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --run-id ID                    Smoke run id (default: timestamped)
  --state-dir DIR                Evidence directory
                                  (default: .agentd/matrix-service-smoke/<run-id>)
  --agentd-bin FILE              agentd binary to run (default: target/debug/agentd)
  --cargo-bin FILE               cargo binary to use for builds (default: cargo)
  --skip-build                   Do not run cargo build before execute
  --agentd-api URL               Existing agentd daemon API URL
                                  (default: AGENTD_MATRIX_AGENTD_API or http://127.0.0.1:8787)
  --iterations N                 Bounded service iterations (default: 1)
  --matrix-homeserver-url URL    Matrix homeserver URL
                                  (or AGENTD_MATRIX_HOMESERVER_URL)
  --matrix-username USER         Matrix username for password login
                                  (or AGENTD_MATRIX_USERNAME)
  --matrix-password PASSWORD     Matrix password for password login
                                  (or AGENTD_MATRIX_PASSWORD)
  --matrix-user-id MXID          Matrix user id for access-token restore
                                  (or AGENTD_MATRIX_USER_ID)
  --matrix-access-token TOKEN    Matrix access token for restore/whoami
                                  (or AGENTD_MATRIX_ACCESS_TOKEN)
  --matrix-device-id ID          Optional Matrix device id for token restore
                                  (or AGENTD_MATRIX_DEVICE_ID)
  --matrix-sync-timeout-ms MS    SDK sync timeout in milliseconds
  --matrix-sdk-store DIR         Optional Matrix SDK SQLite store directory
  --matrix-bot-user-id MXID      Bot MXID used for loop suppression
  --matrix-server-name NAME      Local Matrix server name for puppet MXIDs
  --matrix-agent-prefix PREFIX   Localpart prefix for puppet accounts (default: ac_)
  --matrix-agent NAME            Known local agent name. Repeatable
  --matrix-skip-agent NAME       Known local agent to skip for puppet accounts. Repeatable
  --matrix-trust-mode MODE       audit or enforce (default: audit)
  --matrix-trusted-inviter MXID  Trusted inviter MXID. Repeatable
  --matrix-ignore-sender MXID    Ignored sender MXID. Repeatable
  --matrix-puppet-state FILE     Agent-chat-style bridge-state JSON token file
  --matrix-agent-password-secret SECRET
                                  Secret for deriving puppet account passwords
  --matrix-agent-password-template TEMPLATE
                                  Legacy puppet password template fallback
  --matrix-allow-legacy-agent-password
                                  Allow legacy puppet password template fallback
  --matrix-registration-token TOKEN
                                  Matrix registration token for puppet account UIA
  -h, --help                     Show this help

Environment:
  AGENTD_MATRIX_AGENTS
                                  Optional comma-separated known agent names
  AGENTD_MATRIX_SKIP_AGENTS
                                  Optional comma-separated skipped agent names
  AGENTD_MATRIX_TRUSTED_INVITERS
                                  Optional comma-separated trusted inviter MXIDs
  AGENTD_MATRIX_IGNORED_SENDERS
                                  Optional comma-separated ignored sender MXIDs

Real execution requires AGENTD_REAL_MATRIX_SERVICE_SMOKE=1 and an existing
agentd daemon. Dry-run and preflight-only never start the daemon and never
connect to Matrix or agentd.
EOF
}

trim_space() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s\n' "$value"
}

add_csv_values() {
    local csv="$1"
    local target_name="$2"
    [[ -z "$csv" ]] && return 0
    local values=()
    IFS=',' read -r -a values <<<"$csv"
    local value
    for value in "${values[@]}"; do
        value="$(trim_space "$value")"
        if [[ -n "$value" ]]; then
            eval "$target_name+=(\"\$value\")"
        fi
    done
}

add_csv_values "${AGENTD_MATRIX_AGENTS:-}" MATRIX_AGENTS
add_csv_values "${AGENTD_MATRIX_SKIP_AGENTS:-}" MATRIX_SKIP_AGENTS
add_csv_values "${AGENTD_MATRIX_TRUSTED_INVITERS:-}" MATRIX_TRUSTED_INVITERS
add_csv_values "${AGENTD_MATRIX_IGNORED_SENDERS:-}" MATRIX_IGNORED_SENDERS

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
        --iterations)
            ITERATIONS="${2:?missing --iterations value}"
            shift 2
            ;;
        --matrix-homeserver-url)
            MATRIX_HOMESERVER_URL="${2:?missing --matrix-homeserver-url value}"
            shift 2
            ;;
        --matrix-username)
            MATRIX_USERNAME="${2:?missing --matrix-username value}"
            shift 2
            ;;
        --matrix-password)
            MATRIX_PASSWORD="${2:?missing --matrix-password value}"
            shift 2
            ;;
        --matrix-user-id)
            MATRIX_USER_ID="${2:?missing --matrix-user-id value}"
            shift 2
            ;;
        --matrix-access-token)
            MATRIX_ACCESS_TOKEN="${2:?missing --matrix-access-token value}"
            shift 2
            ;;
        --matrix-device-id)
            MATRIX_DEVICE_ID="${2:?missing --matrix-device-id value}"
            shift 2
            ;;
        --matrix-sync-timeout-ms)
            MATRIX_SYNC_TIMEOUT_MS="${2:?missing --matrix-sync-timeout-ms value}"
            shift 2
            ;;
        --matrix-sdk-store)
            MATRIX_SDK_STORE="${2:?missing --matrix-sdk-store value}"
            shift 2
            ;;
        --matrix-bot-user-id)
            MATRIX_BOT_USER_ID="${2:?missing --matrix-bot-user-id value}"
            shift 2
            ;;
        --matrix-server-name)
            MATRIX_SERVER_NAME="${2:?missing --matrix-server-name value}"
            shift 2
            ;;
        --matrix-agent-prefix)
            MATRIX_AGENT_PREFIX="${2:?missing --matrix-agent-prefix value}"
            shift 2
            ;;
        --matrix-agent)
            MATRIX_AGENTS+=("${2:?missing --matrix-agent value}")
            shift 2
            ;;
        --matrix-skip-agent)
            MATRIX_SKIP_AGENTS+=("${2:?missing --matrix-skip-agent value}")
            shift 2
            ;;
        --matrix-trust-mode)
            MATRIX_TRUST_MODE="${2:?missing --matrix-trust-mode value}"
            shift 2
            ;;
        --matrix-trusted-inviter)
            MATRIX_TRUSTED_INVITERS+=("${2:?missing --matrix-trusted-inviter value}")
            shift 2
            ;;
        --matrix-ignore-sender)
            MATRIX_IGNORED_SENDERS+=("${2:?missing --matrix-ignore-sender value}")
            shift 2
            ;;
        --matrix-puppet-state)
            MATRIX_PUPPET_STATE="${2:?missing --matrix-puppet-state value}"
            shift 2
            ;;
        --matrix-agent-password-secret)
            MATRIX_AGENT_PASSWORD_SECRET="${2:?missing --matrix-agent-password-secret value}"
            shift 2
            ;;
        --matrix-agent-password-template)
            MATRIX_AGENT_PASSWORD_TEMPLATE="${2:?missing --matrix-agent-password-template value}"
            shift 2
            ;;
        --matrix-allow-legacy-agent-password)
            MATRIX_ALLOW_LEGACY_AGENT_PASSWORD="1"
            shift
            ;;
        --matrix-registration-token)
            MATRIX_REGISTRATION_TOKEN="${2:?missing --matrix-registration-token value}"
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
    STATE_DIR="$ROOT/.agentd/matrix-service-smoke/$RUN_ID"
else
    STATE_DIR="$(abs_from_root "$STATE_DIR")"
fi
AGENTD_BIN="$(abs_from_root "$AGENTD_BIN")"
if [[ -n "$MATRIX_SDK_STORE" ]]; then
    MATRIX_SDK_STORE="$(abs_from_root "$MATRIX_SDK_STORE")"
fi
if [[ -n "$MATRIX_PUPPET_STATE" ]]; then
    MATRIX_PUPPET_STATE="$(abs_from_root "$MATRIX_PUPPET_STATE")"
fi

BRIDGE_STATE="$STATE_DIR/matrix-client-bridge-state.json"
PREFLIGHT_OUT="$STATE_DIR/preflight.out"
PREFLIGHT_ERR="$STATE_DIR/preflight.err"
SERVICE_OUT="$STATE_DIR/service.out"
SERVICE_ERR="$STATE_DIR/service.err"
SUMMARY="$STATE_DIR/summary.txt"

secret_status() {
    # Stable redaction markers for evidence review:
    # password: set (redacted)
    # access_token: set (redacted)
    # agent_password_secret: set (redacted)
    # registration_token: set (redacted)
    local value="$1"
    if [[ -n "$value" ]]; then
        printf 'set (redacted)\n'
    else
        printf 'not_set\n'
    fi
}

array_status() {
    local count="$1"
    if [[ "$count" -eq 0 ]]; then
        printf 'none\n'
    else
        printf '%s configured\n' "$count"
    fi
}

print_plan() {
    cat <<EOF
agentd Matrix client bridge service smoke plan
mode: $MODE
repo: $ROOT
run_id: $RUN_ID
state_dir: $STATE_DIR
agentd_bin: $AGENTD_BIN
agentd_api: $AGENTD_API
iterations: $ITERATIONS
matrix_homeserver_url: ${MATRIX_HOMESERVER_URL:-<required>}
username: ${MATRIX_USERNAME:-not_set}
password: $(secret_status "$MATRIX_PASSWORD")
matrix_user_id: ${MATRIX_USER_ID:-not_set}
access_token: $(secret_status "$MATRIX_ACCESS_TOKEN")
matrix_device_id: ${MATRIX_DEVICE_ID:-not_set}
matrix_sdk_store: ${MATRIX_SDK_STORE:-not_set}
matrix_server_name: ${MATRIX_SERVER_NAME:-not_set}
matrix_agent_prefix: $MATRIX_AGENT_PREFIX
matrix_agents: $(array_status "${#MATRIX_AGENTS[@]}")
matrix_skip_agents: $(array_status "${#MATRIX_SKIP_AGENTS[@]}")
trusted_inviters: $(array_status "${#MATRIX_TRUSTED_INVITERS[@]}")
ignored_senders: $(array_status "${#MATRIX_IGNORED_SENDERS[@]}")
agent_password_secret: $(secret_status "$MATRIX_AGENT_PASSWORD_SECRET")
registration_token: $(secret_status "$MATRIX_REGISTRATION_TOKEN")

preflight:
  validate local inputs and run agentd matrix-client-bridge-preflight before service execution

build:
  $CARGO_BIN build -p agentd-bin --features matrix-sdk-adapter

commands:
  agentd matrix-client-bridge-preflight --agentd-api '$AGENTD_API' --state '$BRIDGE_STATE' --iterations '$ITERATIONS' --matrix-homeserver-url '${MATRIX_HOMESERVER_URL:-<required>}'
  agentd matrix-client-bridge-service --agentd-api '$AGENTD_API' --state '$BRIDGE_STATE' --iterations '$ITERATIONS' --matrix-homeserver-url '${MATRIX_HOMESERVER_URL:-<required>}'
  SDK auth flags and puppet-account flags are passed when configured; secret values are redacted from this plan

evidence:
  preflight.out
  preflight.err
  service.out
  service.err
  summary.txt
  matrix-client-bridge-state.json

success criterion:
  agentd matrix-client-bridge-preflight reaches the configured homeserver,
  agentd matrix-client-bridge-service completes the bounded iteration count,
  and the bridge cursor state file is created as service evidence.
EOF
}

need_tool() {
    local tool="$1"
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        return 1
    fi
}

has_value() {
    [[ -n "$(trim_space "$1")" ]]
}

validate_positive_integer() {
    local name="$1"
    local value="$2"
    if [[ ! "$value" =~ ^[0-9]+$ || "$value" -eq 0 ]]; then
        echo "$name must be a positive integer" >&2
        return 2
    fi
}

validate_login_mode() {
    local has_username=0
    local has_password=0
    local has_user_id=0
    local has_access_token=0

    has_value "$MATRIX_USERNAME" && has_username=1
    has_value "$MATRIX_PASSWORD" && has_password=1
    has_value "$MATRIX_USER_ID" && has_user_id=1
    has_value "$MATRIX_ACCESS_TOKEN" && has_access_token=1

    if [[ "$has_username" -ne "$has_password" ]]; then
        echo "Matrix username/password login mode requires both --matrix-username and --matrix-password" >&2
        return 2
    fi
    if [[ "$has_user_id" -ne "$has_access_token" ]]; then
        echo "Matrix user-id/access-token login mode requires both --matrix-user-id and --matrix-access-token" >&2
        return 2
    fi
    if [[ -n "$MATRIX_DEVICE_ID" && "$has_access_token" -eq 0 ]]; then
        echo "--matrix-device-id requires the user-id/access-token login mode" >&2
        return 2
    fi
    if [[ "$has_password" -eq 1 && "$has_access_token" -eq 1 ]]; then
        echo "configure exactly one Matrix SDK login mode: username/password or user-id/access-token" >&2
        return 2
    fi
    if [[ "$has_password" -eq 0 && "$has_access_token" -eq 0 ]]; then
        echo "configure exactly one Matrix SDK login mode: username/password or user-id/access-token" >&2
        return 2
    fi
}

validate_config() {
    if [[ -z "$MATRIX_HOMESERVER_URL" ]]; then
        echo "missing Matrix homeserver URL: pass --matrix-homeserver-url or set AGENTD_MATRIX_HOMESERVER_URL" >&2
        return 2
    fi

    validate_positive_integer "--iterations" "$ITERATIONS"
    validate_login_mode

    case "$(trim_space "$MATRIX_TRUST_MODE")" in
        audit|enforce) ;;
        *)
            echo "--matrix-trust-mode must be audit or enforce" >&2
            return 2
            ;;
    esac

    if [[ "$SKIP_BUILD" != "1" ]]; then
        need_tool "$CARGO_BIN"
    fi

    if [[ "$SKIP_BUILD" == "1" && ! -x "$AGENTD_BIN" ]]; then
        echo "agentd binary is not executable: $AGENTD_BIN" >&2
        return 1
    fi
}

append_repeated_args() {
    local flag="$1"
    shift
    local value
    for value in "$@"; do
        value="$(trim_space "$value")"
        if [[ -n "$value" ]]; then
            AGENTD_COMMON_ARGS+=("$flag" "$value")
        fi
    done
}

build_common_args() {
    AGENTD_COMMON_ARGS=(
        "--agentd-api"
        "$AGENTD_API"
        "--state"
        "$BRIDGE_STATE"
        "--iterations"
        "$ITERATIONS"
        "--matrix-homeserver-url"
        "$MATRIX_HOMESERVER_URL"
        "--matrix-trust-mode"
        "$MATRIX_TRUST_MODE"
        "--matrix-agent-prefix"
        "$MATRIX_AGENT_PREFIX"
        "--matrix-sync-timeout-ms"
        "$MATRIX_SYNC_TIMEOUT_MS"
    )

    if [[ -n "$MATRIX_USERNAME" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-username" "$MATRIX_USERNAME")
    fi
    if [[ -n "$MATRIX_PASSWORD" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-password" "$MATRIX_PASSWORD")
    fi
    if [[ -n "$MATRIX_USER_ID" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-user-id" "$MATRIX_USER_ID")
    fi
    if [[ -n "$MATRIX_ACCESS_TOKEN" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-access-token" "$MATRIX_ACCESS_TOKEN")
    fi
    if [[ -n "$MATRIX_DEVICE_ID" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-device-id" "$MATRIX_DEVICE_ID")
    fi
    if [[ -n "$MATRIX_SDK_STORE" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-sdk-store" "$MATRIX_SDK_STORE")
    fi
    if [[ -n "$MATRIX_BOT_USER_ID" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-bot-user-id" "$MATRIX_BOT_USER_ID")
    fi
    if [[ -n "$MATRIX_SERVER_NAME" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-server-name" "$MATRIX_SERVER_NAME")
    fi
    if (( ${#MATRIX_AGENTS[@]} > 0 )); then
        append_repeated_args "--matrix-agent" "${MATRIX_AGENTS[@]}"
    fi
    if (( ${#MATRIX_SKIP_AGENTS[@]} > 0 )); then
        append_repeated_args "--matrix-skip-agent" "${MATRIX_SKIP_AGENTS[@]}"
    fi
    if (( ${#MATRIX_TRUSTED_INVITERS[@]} > 0 )); then
        append_repeated_args "--matrix-trusted-inviter" "${MATRIX_TRUSTED_INVITERS[@]}"
    fi
    if (( ${#MATRIX_IGNORED_SENDERS[@]} > 0 )); then
        append_repeated_args "--matrix-ignore-sender" "${MATRIX_IGNORED_SENDERS[@]}"
    fi
    if [[ -n "$MATRIX_PUPPET_STATE" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-puppet-state" "$MATRIX_PUPPET_STATE")
    fi
    if [[ -n "$MATRIX_AGENT_PASSWORD_SECRET" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-agent-password-secret" "$MATRIX_AGENT_PASSWORD_SECRET")
    fi
    if [[ -n "$MATRIX_AGENT_PASSWORD_TEMPLATE" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-agent-password-template" "$MATRIX_AGENT_PASSWORD_TEMPLATE")
    fi
    if [[ "$MATRIX_ALLOW_LEGACY_AGENT_PASSWORD" == "1" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-allow-legacy-agent-password")
    fi
    if [[ -n "$MATRIX_REGISTRATION_TOKEN" ]]; then
        AGENTD_COMMON_ARGS+=("--matrix-registration-token" "$MATRIX_REGISTRATION_TOKEN")
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
        echo "iterations: $ITERATIONS"
        echo "matrix_homeserver_url: $MATRIX_HOMESERVER_URL"
        echo "username: ${MATRIX_USERNAME:-not_set}"
        echo "password: $(secret_status "$MATRIX_PASSWORD")"
        echo "matrix_user_id: ${MATRIX_USER_ID:-not_set}"
        echo "access_token: $(secret_status "$MATRIX_ACCESS_TOKEN")"
        echo "matrix_device_id: ${MATRIX_DEVICE_ID:-not_set}"
        echo "matrix_sdk_store: ${MATRIX_SDK_STORE:-not_set}"
        echo "agent_password_secret: $(secret_status "$MATRIX_AGENT_PASSWORD_SECRET")"
        echo "registration_token: $(secret_status "$MATRIX_REGISTRATION_TOKEN")"
        echo "bridge_state: $BRIDGE_STATE"
        echo "preflight_out: $PREFLIGHT_OUT"
        echo "preflight_err: $PREFLIGHT_ERR"
        echo "service_out: $SERVICE_OUT"
        echo "service_err: $SERVICE_ERR"
        echo "agentd_process: external"
    } >"$SUMMARY"
}

run_execute() {
    if [[ "${AGENTD_REAL_MATRIX_SERVICE_SMOKE:-}" != "1" ]]; then
        echo "refusing real Matrix service smoke: set AGENTD_REAL_MATRIX_SERVICE_SMOKE=1 with --execute" >&2
        return 2
    fi

    validate_config
    mkdir -p "$STATE_DIR"

    if [[ "$SKIP_BUILD" != "1" ]]; then
        "$CARGO_BIN" build -p agentd-bin --features matrix-sdk-adapter
    fi

    build_common_args
    if ! "$AGENTD_BIN" "matrix-client-bridge-preflight" "${AGENTD_COMMON_ARGS[@]}" >"$PREFLIGHT_OUT" 2>"$PREFLIGHT_ERR"; then
        write_summary "failed_preflight"
        echo "matrix service smoke preflight failed; evidence: $STATE_DIR" >&2
        return 1
    fi

    if ! "$AGENTD_BIN" "matrix-client-bridge-service" "${AGENTD_COMMON_ARGS[@]}" >"$SERVICE_OUT" 2>"$SERVICE_ERR"; then
        write_summary "failed_service"
        echo "matrix service smoke failed; evidence: $STATE_DIR" >&2
        return 1
    fi

    if [[ ! -e "$BRIDGE_STATE" ]]; then
        write_summary "failed_missing_state"
        echo "Matrix service smoke did not create bridge cursor state: $BRIDGE_STATE" >&2
        return 1
    fi

    write_summary "finished"
    echo "matrix service smoke finished; evidence: $STATE_DIR"
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
