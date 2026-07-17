#!/usr/bin/env bash
# Render and structurally gate the checked-in enterprise Kubernetes profile.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KUSTOMIZATION="$ROOT/deploy/enterprise"
OUTPUT=""
ALLOW_REFERENCE_PLACEHOLDERS=0
SERVER_DRY_RUN=0

usage() {
    cat <<'EOF'
usage: agentd_enterprise_deploy_preflight.sh [options]

Options:
  --kustomization DIR          Kustomization directory (default: deploy/enterprise)
  --output FILE                New rendered-manifest evidence file
  --allow-reference-placeholders
                               Permit checked-in non-production placeholders
  --server-dry-run             Submit the render with kubectl apply --dry-run=server

The profile must render one SQLite control-plane replica with one RWO PVC,
six zone worker deployments with three replicas and hard hostname spreading,
and six matching HPAs. Acceptance must not allow reference placeholders.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --kustomization) KUSTOMIZATION="${2:?missing --kustomization value}"; shift 2 ;;
        --output) OUTPUT="${2:?missing --output value}"; shift 2 ;;
        --allow-reference-placeholders) ALLOW_REFERENCE_PLACEHOLDERS=1; shift ;;
        --server-dry-run) SERVER_DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

for tool in kubectl yq rg shasum; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "missing required tool: $tool" >&2
        exit 2
    fi
done
if [[ "$KUSTOMIZATION" != /* ]]; then
    KUSTOMIZATION="$ROOT/$KUSTOMIZATION"
fi
if [[ ! -f "$KUSTOMIZATION/kustomization.yaml" ]]; then
    echo "kustomization is unavailable: $KUSTOMIZATION" >&2
    exit 2
fi
if [[ -z "$OUTPUT" ]]; then
    OUTPUT="$ROOT/.agentd/enterprise-preflight/$(date -u +%Y%m%dT%H%M%SZ)/rendered.yaml"
elif [[ "$OUTPUT" != /* ]]; then
    OUTPUT="$ROOT/$OUTPUT"
fi
if [[ -e "$OUTPUT" || -e "$OUTPUT.sha256" ]]; then
    echo "refusing to overwrite preflight evidence: $OUTPUT" >&2
    exit 2
fi

TMP_RENDER="$(mktemp "${TMPDIR:-/tmp}/agentd-enterprise-render.XXXXXX")"
cleanup() { rm -f "$TMP_RENDER"; }
trap cleanup EXIT
kubectl kustomize "$KUSTOMIZATION" >"$TMP_RENDER"

scalar() {
    local expression="$1"
    yq ea -r "$expression" "$TMP_RENDER"
}

CONTROL_PLANE_REPLICAS="$(scalar 'select(.kind == "Deployment" and .metadata.name == "agentd-control-plane") | .spec.replicas')"
if [[ "$CONTROL_PLANE_REPLICAS" != "1" ]]; then
    echo "reference profile must render exactly one control-plane replica" >&2
    exit 1
fi
CONTROL_PLANE_SECURITY="$(scalar '
    select(.kind == "Deployment" and .metadata.name == "agentd-control-plane") |
    select(
        .spec.template.spec.securityContext.runAsNonRoot == true and
        .spec.template.spec.securityContext.runAsUser == 65532 and
        .spec.template.spec.securityContext.runAsGroup == 65532 and
        .spec.template.spec.securityContext.fsGroup == 65532
    ) | "valid"
')"
if [[ "$CONTROL_PLANE_SECURITY" != "valid" ]]; then
    echo "control plane must use the fixed non-root UID/GID/fsGroup 65532" >&2
    exit 1
fi
CONTROL_PLANE_HEALTH="$(scalar '
    select(.kind == "Deployment" and .metadata.name == "agentd-control-plane") |
    .spec.template.spec.containers[] |
    select(.name == "envoy") |
    select(
        .readinessProbe.httpGet.path == "/healthz" and
        .readinessProbe.httpGet.scheme == "HTTPS" and
        .livenessProbe.httpGet.path == "/healthz" and
        .livenessProbe.httpGet.scheme == "HTTPS"
    ) | "valid"
')"
if [[ "$CONTROL_PLANE_HEALTH" != "valid" ]]; then
    echo "Envoy probes must cover agentd through the operator TLS /healthz route" >&2
    exit 1
fi
PVC_ACCESS="$(scalar 'select(.kind == "PersistentVolumeClaim" and .metadata.name == "agentd-control-plane-reference-state") | .spec.accessModes[]')"
if [[ "$PVC_ACCESS" != "ReadWriteOnce" ]]; then
    echo "reference control-plane state must use one ReadWriteOnce PVC" >&2
    exit 1
fi

EXPECTED_WORKERS="$(printf '%s\n' \
    agentd-worker-cn-east-1a agentd-worker-cn-east-1b agentd-worker-cn-east-1c \
    agentd-worker-cn-north-1a agentd-worker-cn-north-1b agentd-worker-cn-north-1c)"
ACTUAL_WORKERS="$(scalar 'select(.kind == "Deployment" and (.metadata.name | startswith("agentd-worker-"))) | .metadata.name' | sort)"
if [[ "$ACTUAL_WORKERS" != "$EXPECTED_WORKERS" ]]; then
    echo "enterprise profile does not contain the exact six worker zone pools" >&2
    exit 1
fi
INVALID_WORKER_REPLICAS="$(scalar 'select(.kind == "Deployment" and (.metadata.name | startswith("agentd-worker-")) and .spec.replicas != 3) | .metadata.name')"
if [[ -n "$INVALID_WORKER_REPLICAS" ]]; then
    echo "worker pools must start with exactly three replicas: $INVALID_WORKER_REPLICAS" >&2
    exit 1
fi
INVALID_WORKER_SECURITY="$(scalar '
    select(.kind == "Deployment" and (.metadata.name | startswith("agentd-worker-"))) |
    select(
        .spec.template.spec.securityContext.runAsNonRoot != true or
        .spec.template.spec.securityContext.runAsUser != 65532 or
        .spec.template.spec.securityContext.runAsGroup != 65532 or
        .spec.template.spec.securityContext.fsGroup != 65532
    ) | .metadata.name
')"
if [[ -n "$INVALID_WORKER_SECURITY" ]]; then
    echo "worker pools must use the fixed non-root UID/GID/fsGroup 65532: $INVALID_WORKER_SECURITY" >&2
    exit 1
fi
INVALID_WORKER_PLACEMENT="$(scalar '
    select(.kind == "Deployment" and (.metadata.name | startswith("agentd-worker-"))) |
    select(
        .spec.template.spec.nodeSelector."topology.kubernetes.io/region" != .metadata.labels."agentd.dev/region" or
        .spec.template.spec.nodeSelector."topology.kubernetes.io/zone" != .metadata.labels."agentd.dev/zone"
    ) | .metadata.name
')"
if [[ -n "$INVALID_WORKER_PLACEMENT" ]]; then
    echo "worker node placement must match its declared region and zone: $INVALID_WORKER_PLACEMENT" >&2
    exit 1
fi
INVALID_WORKER_SPREAD="$(scalar '
    select(.kind == "Deployment" and (.metadata.name | startswith("agentd-worker-"))) |
    select(([
        .spec.template.spec.topologySpreadConstraints[]? |
        select(.topologyKey == "kubernetes.io/hostname" and .whenUnsatisfiable == "DoNotSchedule")
    ] | length) != 1) | .metadata.name
')"
if [[ -n "$INVALID_WORKER_SPREAD" ]]; then
    echo "every worker pool requires exactly one hard hostname topology spread constraint: $INVALID_WORKER_SPREAD" >&2
    exit 1
fi
ACTUAL_HPAS="$(scalar 'select(.kind == "HorizontalPodAutoscaler" and (.metadata.name | startswith("agentd-worker-"))) | .metadata.name' | sort)"
if [[ "$ACTUAL_HPAS" != "$EXPECTED_WORKERS" ]]; then
    echo "enterprise profile requires one exactly named HPA for every worker pool" >&2
    exit 1
fi
INVALID_HPA_TARGETS="$(scalar '
    select(.kind == "HorizontalPodAutoscaler" and (.metadata.name | startswith("agentd-worker-"))) |
    select(
        .spec.scaleTargetRef.apiVersion != "apps/v1" or
        .spec.scaleTargetRef.kind != "Deployment" or
        .spec.scaleTargetRef.name != .metadata.name or
        .spec.minReplicas != 3
    ) | .metadata.name
')"
if [[ -n "$INVALID_HPA_TARGETS" ]]; then
    echo "worker HPAs must target their same-named deployment with minReplicas=3: $INVALID_HPA_TARGETS" >&2
    exit 1
fi

if [[ "$ALLOW_REFERENCE_PLACEHOLDERS" != "1" ]] && rg -n \
    'example\.invalid|factory\.example|REPLACE_WITH|replace-with|replace_me|replace-me|_replace_with_|sha256:0{64}|kms(-version)?://[^[:space:]]*/replace' \
    "$TMP_RENDER"; then
    echo "enterprise acceptance refuses reference placeholders" >&2
    exit 1
fi

if [[ "$SERVER_DRY_RUN" == "1" ]]; then
    kubectl apply --dry-run=server -f "$TMP_RENDER" >/dev/null
fi

mkdir -m 0700 -p "$(dirname "$OUTPUT")"
mv "$TMP_RENDER" "$OUTPUT"
trap - EXIT
chmod a-w "$OUTPUT"
RENDER_SHA="$(shasum -a 256 "$OUTPUT" | awk '{print $1}')"
printf '%s  %s\n' "$RENDER_SHA" "$(basename "$OUTPUT")" >"$OUTPUT.sha256"
chmod a-w "$OUTPUT.sha256"
printf 'rendered=%s\nsha256_file=%s\nsha256=%s\nserver_dry_run=%s\n' \
    "$OUTPUT" "$OUTPUT.sha256" "$RENDER_SHA" "$SERVER_DRY_RUN"
