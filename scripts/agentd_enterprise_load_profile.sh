#!/usr/bin/env bash
# Guarded AD-E7 factory load-profile harness. The driver emits metrics JSON only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
MODEL="$ROOT/config/enterprise/factory-load-model-v1.json"
DRIVER=""
EVIDENCE_DIR=""
DRIVER_ARGS=()
MAX_DRIVER_STREAM_BYTES=$((2 * 1024 * 1024))
SCRATCH_DIR=""

usage() {
    cat <<'EOF'
usage: agentd_enterprise_load_profile.sh [--dry-run|--execute] --driver PATH [options]

Options:
  --model FILE          Pinned load-model JSON (default: config/enterprise/factory-load-model-v1.json)
  --driver PATH         Executable load driver; it must emit one metrics JSON document on stdout
  --driver-arg VALUE    Repeatable opaque driver argument
  --evidence-dir DIR    New immutable evidence directory

Execution requires AGENTD_ENTERPRISE_LOAD_PROFILE=1 and --execute. Claude
credentials are removed from the driver environment. Prompts, transcripts,
credentials, secret bytes, tenant keys, and artifact bytes are forbidden in
driver output. The driver receives a cleared environment with only documented
AGENTD connection variables, PATH, HOME, and TMPDIR forwarded.

Metrics stdout must be one agentd.enterprise-load-metrics/v1 JSON object with
the exact loadModelSha256 and a dimensions object containing tenant, project,
room, matrixEvent, queue, artifactLog, certificationThroughput,
failureInjection, testWindow, noisyNeighbor, and budget results.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) MODE="dry-run"; shift ;;
        --execute) MODE="execute"; shift ;;
        --model) MODEL="${2:?missing --model value}"; shift 2 ;;
        --driver) DRIVER="${2:?missing --driver value}"; shift 2 ;;
        --driver-arg) DRIVER_ARGS+=("${2:?missing --driver-arg value}"); shift 2 ;;
        --evidence-dir) EVIDENCE_DIR="${2:?missing --evidence-dir value}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

sha256_file() {
    shasum -a 256 "$1" | awk '{print $1}'
}

if [[ ! -f "$MODEL" || ! -s "$MODEL" ]]; then
    echo "load model is unavailable: $MODEL" >&2
    exit 2
fi
if [[ -z "$DRIVER" || ! -f "$DRIVER" || ! -x "$DRIVER" ]]; then
    echo "--driver must name an executable regular file" >&2
    exit 2
fi
if [[ -z "$EVIDENCE_DIR" ]]; then
    EVIDENCE_DIR="$ROOT/.agentd/enterprise-load/$(date -u +%Y%m%dT%H%M%SZ)"
elif [[ "$EVIDENCE_DIR" != /* ]]; then
    EVIDENCE_DIR="$ROOT/$EVIDENCE_DIR"
fi
if [[ -e "$EVIDENCE_DIR" ]]; then
    echo "evidence directory already exists: $EVIDENCE_DIR" >&2
    exit 2
fi

MODEL_SHA="$(sha256_file "$MODEL")"
DRIVER_SHA="$(sha256_file "$DRIVER")"
printf 'mode=%s\nmodel=%s\nmodel_sha256=%s\ndriver=%s\ndriver_sha256=%s\nevidence=%s\n' \
    "$MODE" "$MODEL" "$MODEL_SHA" "$DRIVER" "$DRIVER_SHA" "$EVIDENCE_DIR"

if [[ "$MODE" != "execute" ]]; then
    exit 0
fi
if [[ "${AGENTD_ENTERPRISE_LOAD_PROFILE:-}" != "1" ]]; then
    echo "refusing load execution: set AGENTD_ENTERPRISE_LOAD_PROFILE=1" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "refusing load execution: jq is required for bounded metrics validation" >&2
    exit 2
fi

mkdir -m 0700 -p "$EVIDENCE_DIR"
SCRATCH_DIR="$(mktemp -d "${TMPDIR:-/tmp}/agentd-enterprise-load.XXXXXX")"
cp "$MODEL" "$SCRATCH_DIR/load-model.json"
chmod 0400 "$SCRATCH_DIR/load-model.json"
cleanup() {
    if [[ -n "$SCRATCH_DIR" && -d "$SCRATCH_DIR" ]]; then
        rm -rf "$SCRATCH_DIR"
    fi
}
trap cleanup EXIT
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
set +e
(
    ulimit -f 4096
    env -i \
        PATH="${PATH:-/usr/bin:/bin}" \
        HOME="${HOME:-/nonexistent}" \
        TMPDIR="${TMPDIR:-/tmp}" \
        AGENTD_ENTERPRISE_URL="${AGENTD_ENTERPRISE_URL:-}" \
        AGENTD_API_TOKEN="${AGENTD_API_TOKEN:-}" \
        AGENTD_OPERATOR_CA_PEM="${AGENTD_OPERATOR_CA_PEM:-}" \
        AGENTD_FACTORY_LOAD_MODEL="$SCRATCH_DIR/load-model.json" \
        AGENTD_FACTORY_LOAD_MODEL_SHA256="$MODEL_SHA" \
        AGENTD_LOAD_SCRATCH_DIR="$SCRATCH_DIR" \
        KUBECONFIG="${KUBECONFIG:-}" \
        "$DRIVER" "${DRIVER_ARGS[@]}" \
        >"$SCRATCH_DIR/metrics.json" 2>"$SCRATCH_DIR/driver.stderr"
)
DRIVER_STATUS=$?
set -e
COMPLETED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

METRICS_VALID=1
for stream in metrics.json driver.stderr; do
    if [[ ! -f "$SCRATCH_DIR/$stream" ]] || \
       [[ "$(wc -c <"$SCRATCH_DIR/$stream")" -gt "$MAX_DRIVER_STREAM_BYTES" ]]; then
        METRICS_VALID=0
    fi
done
if [[ ! -s "$SCRATCH_DIR/metrics.json" ]] || ! jq -e --arg model "$MODEL_SHA" '
    . as $root |
    ["tenant", "project", "room", "matrixEvent", "queue", "artifactLog",
     "certificationThroughput", "failureInjection", "testWindow",
     "noisyNeighbor", "budget"] as $required |
    type == "object" and
    $root.schemaVersion == "agentd.enterprise-load-metrics/v1" and
    $root.loadModelSha256 == $model and
    ($root.dimensions | type == "object") and
    all($required[]; ($root.dimensions[.] | type == "object")) and
    ([$root | .. | objects | keys[]] | all(
      test("(^|_)(prompt|transcript|credential|secret|private_?key|certificate|artifact_?bytes|raw_?content|stdout|stderr)($|_)"; "i") | not
    ))
' "$SCRATCH_DIR/metrics.json" >/dev/null 2>&1; then
    METRICS_VALID=0
fi
if [[ "$METRICS_VALID" != "1" ]]; then
    DRIVER_STATUS=2
    printf '%s\n' '{"error":"driver metrics rejected by evidence policy"}' \
        >"$EVIDENCE_DIR/metrics.json"
else
    cp "$SCRATCH_DIR/metrics.json" "$EVIDENCE_DIR/metrics.json"
fi
cp "$MODEL" "$EVIDENCE_DIR/load-model.json"
METRICS_SHA="$(sha256_file "$EVIDENCE_DIR/metrics.json")"
if [[ -f "$SCRATCH_DIR/driver.stderr" ]]; then
    STDERR_SHA="$(sha256_file "$SCRATCH_DIR/driver.stderr")"
else
    STDERR_SHA="$(printf '' | shasum -a 256 | awk '{print $1}')"
fi
SOURCE_REV="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')"
cat >"$EVIDENCE_DIR/manifest.json" <<EOF
{
  "schemaVersion": "agentd.enterprise-load-evidence/v1",
  "sourceRevision": "$SOURCE_REV",
  "loadModelSha256": "$MODEL_SHA",
  "driverSha256": "$DRIVER_SHA",
  "metricsSha256": "$METRICS_SHA",
  "stderrSha256": "$STDERR_SHA",
  "stderrRetained": false,
  "metricsPolicyAccepted": $([[ "$METRICS_VALID" == "1" ]] && printf true || printf false),
  "driverExitStatus": $DRIVER_STATUS,
  "startedAt": "$STARTED_AT",
  "completedAt": "$COMPLETED_AT"
}
EOF
MANIFEST_SHA="$(sha256_file "$EVIDENCE_DIR/manifest.json")"
printf '%s  manifest.json\n' "$MANIFEST_SHA" >"$EVIDENCE_DIR/MANIFEST.sha256"
chmod -R a-w "$EVIDENCE_DIR"
printf 'driver_status=%s\nmetrics_sha256=%s\nmanifest_sha256=%s\n' \
    "$DRIVER_STATUS" "$METRICS_SHA" "$MANIFEST_SHA"
exit "$DRIVER_STATUS"
