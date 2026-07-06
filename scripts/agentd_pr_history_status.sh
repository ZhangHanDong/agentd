#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -gt 2 ]; then
  echo "usage: $0 [head_ref] [base_branch]" >&2
  exit 64
fi

head_ref=${1:-HEAD}
base_branch=${2:-main}
remote=origin

if ! printf '%s' "$head_ref" | grep -Eq '^(HEAD|[A-Za-z0-9][A-Za-z0-9._/-]*)$'; then
  echo "invalid head ref for PR history status: $head_ref" >&2
  exit 64
fi

if ! printf '%s' "$base_branch" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._/-]*$'; then
  echo "invalid base branch for PR history status: $base_branch" >&2
  exit 64
fi

base_ref="$remote/$base_branch"

if ! git fetch "$remote" "+refs/heads/$base_branch:refs/remotes/$remote/$base_branch" >&2; then
  echo "git prerequisite failed: unable to fetch $base_ref" >&2
  exit 66
fi

if ! head_sha=$(git rev-parse --verify --quiet "$head_ref^{commit}"); then
  echo "git prerequisite failed: head ref not found: $head_ref" >&2
  exit 66
fi

if ! base_sha=$(git rev-parse --verify --quiet "$base_ref^{commit}"); then
  echo "git prerequisite failed: base ref not found after fetch: $base_ref" >&2
  exit 66
fi

printf 'head_ref: %s\n' "$head_ref"
printf 'head_sha: %s\n' "$head_sha"
printf 'base_ref: %s\n' "$base_ref"
printf 'base_sha: %s\n' "$base_sha"

if ! merge_base=$(git merge-base "$base_ref" "$head_ref" 2>/dev/null); then
  printf 'merge_base: none\n'
  echo "cannot open PR: $head_ref has no common history with $base_ref" >&2
  exit 65
fi

printf 'merge_base: %s\n' "$merge_base"
