#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <spec-path>" >&2
    exit 64
fi

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to inspect agent-spec metadata" >&2
    exit 69
fi

spec=$1
parsed=$(agent-spec parse "$spec" --format json)
verification=$(jq -r '
    if (.meta.tags | index("design-only")) != null then "design-only"
    elif (.meta.tags | index("template-only")) != null then "template-only"
    else "lifecycle"
    end
' <<<"$parsed")
if [[ "$verification" != "lifecycle" ]]; then
    echo "verification=$verification path=$spec"
    agent-spec lint "$spec" --min-score 0.7 --format text
else
    echo "verification=lifecycle path=$spec"
    agent-spec lifecycle "$spec" --code . --min-score 0.7 --format text
fi
