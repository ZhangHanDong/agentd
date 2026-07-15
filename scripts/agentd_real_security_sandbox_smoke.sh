#!/usr/bin/env bash
# Opt-in real OCI isolation smoke. Dry-run is the default and starts no container.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
RUNTIME="${AGENTD_SECURITY_SANDBOX_RUNTIME:-auto}"
IMAGE="${AGENTD_SECURITY_SANDBOX_IMAGE:-}"
STATE_DIR=""

usage() {
    cat <<'EOF'
usage: agentd_real_security_sandbox_smoke.sh [--dry-run|--preflight-only|--execute] [options]

Options:
  --runtime auto|docker|podman
  --image IMAGE@sha256:DIGEST
  --state-dir DIR

Execute mode requires AGENTD_REAL_SECURITY_SANDBOX_SMOKE=1 and a preloaded,
immutable test image containing sh plus curl or wget. No agent runtime or
external service is started.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) MODE="dry-run"; shift ;;
        --preflight-only) MODE="preflight-only"; shift ;;
        --execute) MODE="execute"; shift ;;
        --runtime) RUNTIME="${2:?missing --runtime value}"; shift 2 ;;
        --image) IMAGE="${2:?missing --image value}"; shift 2 ;;
        --state-dir) STATE_DIR="${2:?missing --state-dir value}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ -z "$STATE_DIR" ]]; then
    STATE_DIR="$ROOT/.agentd/real-security-sandbox-smoke"
elif [[ "$STATE_DIR" != /* ]]; then
    STATE_DIR="$ROOT/$STATE_DIR"
fi

select_runtime() {
    case "$RUNTIME" in
        docker|podman) printf '%s\n' "$RUNTIME" ;;
        auto)
            if command -v docker >/dev/null 2>&1; then
                printf 'docker\n'
            elif command -v podman >/dev/null 2>&1; then
                printf 'podman\n'
            else
                echo "docker or podman is required" >&2
                return 1
            fi
            ;;
        *) echo "invalid runtime: $RUNTIME" >&2; return 2 ;;
    esac
}

print_plan() {
    cat <<EOF
agentd real security sandbox smoke plan
mode: $MODE
runtime: $RUNTIME
image: ${IMAGE:-<required-for-preflight-and-execute>}
state_dir: $STATE_DIR
controls:
  immutable image digest
  read-only root and dropped capabilities
  no-new-privileges and runtime-default seccomp
  bounded pids, memory, and CPU
  tenant-A workspace/cache mounts only
  no host credentials, tenant-B workspace, or shared cache mounts
  network none with an in-container public egress probe
EOF
}

print_plan
if [[ "$MODE" == "dry-run" ]]; then
    exit 0
fi

if [[ -z "$IMAGE" || ! "$IMAGE" =~ @sha256:[0-9a-f]{64}$ ]]; then
    echo "--image must be an immutable IMAGE@sha256:DIGEST reference" >&2
    exit 2
fi
if [[ "$MODE" == "execute" && "${AGENTD_REAL_SECURITY_SANDBOX_SMOKE:-}" != "1" ]]; then
    echo "execute requires AGENTD_REAL_SECURITY_SANDBOX_SMOKE=1" >&2
    exit 2
fi
OCI_RUNTIME="$(select_runtime)"
command -v "$OCI_RUNTIME" >/dev/null 2>&1
"$OCI_RUNTIME" image inspect "$IMAGE" >/dev/null

if [[ "$MODE" == "preflight-only" ]]; then
    echo "preflight ok"
    exit 0
fi
RUNTIME_STATE="$STATE_DIR/runtime"
TENANT_A="$RUNTIME_STATE/tenant-a"
TENANT_B="$RUNTIME_STATE/tenant-b"
TENANT_CACHE="$RUNTIME_STATE/cache-tenant-a"
SHARED_CACHE="$RUNTIME_STATE/cache-shared"
HOST_CREDENTIALS="$RUNTIME_STATE/host-credentials"
OUTPUT="$RUNTIME_STATE/output"
rm -rf "$RUNTIME_STATE"
mkdir -p "$TENANT_A" "$TENANT_B" "$TENANT_CACHE" "$SHARED_CACHE" "$HOST_CREDENTIALS" "$OUTPUT"
printf 'tenant-a\n' >"$TENANT_A/input.txt"
printf 'tenant-b-private\n' >"$TENANT_B/private.txt"
printf 'shared-cache-private\n' >"$SHARED_CACHE/private.txt"
printf 'host-credential-private\n' >"$HOST_CREDENTIALS/credentials"

"$OCI_RUNTIME" run --rm \
    --read-only \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --security-opt seccomp=runtime-default \
    --pids-limit 64 \
    --memory 268435456 \
    --cpus 1.000 \
    --network none \
    --mount "type=bind,src=$TENANT_A,dst=/workspace/input,readonly" \
    --mount "type=bind,src=$TENANT_CACHE,dst=/cache" \
    --mount "type=bind,src=$OUTPUT,dst=/output" \
    "$IMAGE" \
    sh -ceu '
        test -r /workspace/input/input.txt
        test ! -e /tenant-b
        test ! -e /shared-cache
        test ! -e /host-credentials
        test ! -e /root/.aws/credentials
        if command -v wget >/dev/null 2>&1; then
            ! wget -T 2 -q -O /dev/null https://example.com
        elif command -v curl >/dev/null 2>&1; then
            ! curl --max-time 2 -fsS -o /dev/null https://example.com
        else
            echo "test image lacks curl or wget" >&2
            exit 22
        fi
        printf retained > /output/result.txt
    '

test "$(cat "$OUTPUT/result.txt")" = "retained"
mkdir -p "$STATE_DIR"
cp "$OUTPUT/result.txt" "$STATE_DIR/retained-output.txt"
rm -rf "$RUNTIME_STATE"
test ! -e "$RUNTIME_STATE"
cat >"$STATE_DIR/summary.txt" <<EOF
result: passed
runtime: $OCI_RUNTIME
image: $IMAGE
host_credentials: denied
cross_tenant_workspace: denied
shared_cache: denied
public_egress: denied
transient_runtime_state_removed: true
EOF
echo "sandbox smoke passed: $STATE_DIR/summary.txt"
