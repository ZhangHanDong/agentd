#!/usr/bin/env bash
set -euo pipefail

case "${1:-}" in
    --staged|--head)
        [ "$#" -eq 1 ] || {
            echo "usage: $0 <--staged|--head|--range BASE>" >&2
            exit 64
        }
        ;;
    --range)
        [ "$#" -eq 2 ] && [ -n "$2" ] || {
            echo "usage: $0 <--staged|--head|--range BASE>" >&2
            exit 64
        }
        ;;
    *)
        echo "usage: $0 <--staged|--head|--range BASE>" >&2
        exit 64
        ;;
esac

if ! command -v agent-spec >/dev/null 2>&1; then
    echo "agent-spec is required for changed-contract guard" >&2
    exit 69
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required for changed-contract guard" >&2
    exit 69
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

changes=()
if [ "$1" = "--staged" ]; then
    while IFS= read -r -d '' path; do
        [ -n "$path" ] && changes+=("$path")
    done < <(git diff --cached --name-only -z --no-renames --diff-filter=ACMRDT)
elif [ "$1" = "--head" ]; then
    while IFS= read -r -d '' path; do
        [ -n "$path" ] && changes+=("$path")
    done < <(git diff-tree --root --no-commit-id --name-only -z --no-renames --diff-filter=ACMRDT -r HEAD)
else
    requested_base=$2
    adoption_path=specs/e2e/p156-portable-protected-checks.spec.md
    if git cat-file -e "$requested_base:$adoption_path" 2>/dev/null; then
        range_base=$requested_base
    else
        adoption_commit=$(
            git rev-list --reverse "$requested_base"..HEAD -- "$adoption_path" | sed -n '1p'
        )
        if [ -z "$adoption_commit" ]; then
            echo "changed-contract guard: P156 adoption is absent from base and range" >&2
            exit 1
        fi
        range_base="$adoption_commit^"
        echo "changed-contract guard: bootstrap range starts at P156 adoption"
    fi
    while IFS= read -r -d '' path; do
        [ -n "$path" ] && changes+=("$path")
    done < <(git diff "$range_base"...HEAD --name-only -z --no-renames --diff-filter=ACMRDT)
fi

if [ "${#changes[@]}" -eq 0 ]; then
    echo "changed-contract guard: no changes"
    exit 0
fi

candidates=()
for path in "${changes[@]}"; do
    case "$path" in
        specs/*.spec|specs/*.spec.md)
            [ ! -f "$path" ] || candidates+=("$path")
            ;;
    esac
done

if [ "${#candidates[@]}" -eq 0 ]; then
    echo "changed-contract guard: no changed implementation contract governs this delta" >&2
    exit 1
fi

implementation_candidates=0
last_output=
for spec in "${candidates[@]}"; do
    parsed=$(agent-spec parse "$spec" --format json)
    verification=$(jq -r '
        if (.meta.tags | index("design-only")) != null then "design-only"
        elif (.meta.tags | index("template-only")) != null then "template-only"
        else "lifecycle"
        end
    ' <<<"$parsed")
    if [ "$verification" != "lifecycle" ]; then
        echo "changed-contract guard: skipping $verification contract $spec"
        continue
    fi

    implementation_candidates=$((implementation_candidates + 1))
    # Build one agent-spec lifecycle invocation with every explicit change.
    lifecycle_args=(lifecycle "$spec" --code . --min-score 0.7 --format text)
    for path in "${changes[@]}"; do
        lifecycle_args+=(--change "$path")
    done
    if output=$(agent-spec "${lifecycle_args[@]}" 2>&1); then
        echo "changed-contract guard: PASS $spec (${#changes[@]} changes)"
        exit 0
    fi
    last_output=$output
    echo "changed-contract guard: rejected candidate $spec" >&2
done

if [ "$implementation_candidates" -eq 0 ]; then
    echo "changed-contract guard: no changed implementation contract governs this delta" >&2
else
    echo "changed-contract guard: no implementation contract accepted every change" >&2
    [ -z "$last_output" ] || printf '%s\n' "$last_output" >&2
fi
exit 1
