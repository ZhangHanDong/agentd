#!/usr/bin/env bash
set -euo pipefail

mode="dry-run"
base_branch="main"
remote="origin"

usage() {
  cat <<'EOF'
usage: agentd_pr_history_bridge.sh [--dry-run|--execute] [base_branch]

Create a local merge-base between the current HEAD and origin/<base_branch>
without pushing or rewriting history. Real execution requires:
  AGENTD_PR_HISTORY_BRIDGE=1 bash scripts/agentd_pr_history_bridge.sh --execute
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      mode="dry-run"
      shift
      ;;
    --execute)
      mode="execute"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --*)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
    *)
      if [ "$base_branch" != "main" ]; then
        echo "unexpected extra argument: $1" >&2
        usage >&2
        exit 64
      fi
      base_branch="$1"
      shift
      ;;
  esac
done

if ! printf '%s' "$base_branch" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._/-]*$'; then
  echo "invalid base branch for PR history bridge: $base_branch" >&2
  exit 64
fi

base_ref="$remote/$base_branch"

if [ "$mode" = "execute" ] && [ "${AGENTD_PR_HISTORY_BRIDGE:-}" != "1" ]; then
  echo "refusing PR history bridge: set AGENTD_PR_HISTORY_BRIDGE=1 with --execute" >&2
  exit 2
fi

git fetch "$remote" "+refs/heads/$base_branch:refs/remotes/$remote/$base_branch" >&2

if ! git rev-parse --verify --quiet "HEAD^{commit}" >/dev/null; then
  echo "git prerequisite failed: HEAD is not a commit" >&2
  exit 66
fi

if ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
  echo "git prerequisite failed: base ref not found after fetch: $base_ref" >&2
  exit 66
fi

printf 'mode: %s\n' "$mode"
printf 'head_ref: HEAD\n'
printf 'base_ref: %s\n' "$base_ref"

if merge_base=$(git merge-base "$base_ref" HEAD 2>/dev/null); then
  printf 'merge_required: no\n'
  printf 'merge_base: %s\n' "$merge_base"
  exit 0
fi

printf 'merge_required: yes\n'
printf 'command: git merge --allow-unrelated-histories --no-edit %s\n' "$base_ref"

if [ "$mode" = "dry-run" ]; then
  exit 0
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "dirty worktree: commit or stash changes before PR history bridge" >&2
  exit 65
fi

git merge --allow-unrelated-histories --no-edit "$base_ref"

merge_base=$(git merge-base "$base_ref" HEAD)
printf 'merge_base: %s\n' "$merge_base"
