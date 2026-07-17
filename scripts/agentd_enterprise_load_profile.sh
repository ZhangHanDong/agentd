#!/usr/bin/env bash
# Guarded AD-E7 factory load-profile harness. The driver emits metrics JSON only.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="dry-run"
MODEL="$ROOT/config/enterprise/factory-load-model-v1.json"
DRIVER=""
EVIDENCE_DIR=""
DRIVER_ARGS=()

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
driver output.
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

mkdir -m 0700 -p "$EVIDENCE_DIR"
cp "$MODEL" "$EVIDENCE_DIR/load-model.json"
STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
set +e
env -u ANTHROPIC_API_KEY -u CLAUDE_API_KEY \
    AGENTD_FACTORY_LOAD_MODEL="$EVIDENCE_DIR/load-model.json" \
    AGENTD_FACTORY_LOAD_MODEL_SHA256="$MODEL_SHA" \
    AGENTD_LOAD_EVIDENCE_DIR="$EVIDENCE_DIR" \
    "$DRIVER" "${DRIVER_ARGS[@]}" >"$EVIDENCE_DIR/metrics.json" 2>"$EVIDENCE_DIR/driver.stderr"
DRIVER_STATUS=$?
set -e
COMPLETED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if [[ ! -s "$EVIDENCE_DIR/metrics.json" ]]; then
    echo '{"error":"driver produced no metrics"}' >"$EVIDENCE_DIR/metrics.json"
fi
METRICS_SHA="$(sha256_file "$EVIDENCE_DIR/metrics.json")"
STDERR_SHA="$(sha256_file "$EVIDENCE_DIR/driver.stderr")"
SOURCE_REV="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unknown')"
cat >"$EVIDENCE_DIR/manifest.json" <<EOF
{
  "schemaVersion": "agentd.enterprise-load-evidence/v1",
  "sourceRevision": "$SOURCE_REV",
  "loadModelSha256": "$MODEL_SHA",
  "driverSha256": "$DRIVER_SHA",
  "metricsSha256": "$METRICS_SHA",
  "stderrSha256": "$STDERR_SHA",
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
