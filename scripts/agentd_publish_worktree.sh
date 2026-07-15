#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 4 ]; then
  echo "usage: $0 <worktree> <task_run_id> [base_commit] [report_path]" >&2
  exit 64
fi

worktree=$1
task_run_id=$2
base_commit=${3:-}
report=${4:-.agentd/run/report.md}

if [ ! -d "$worktree" ]; then
  echo "worktree does not exist: $worktree" >&2
  exit 66
fi

if ! printf '%s' "$task_run_id" | grep -Eq '^tr_[0-9A-HJKMNP-TV-Z]{26}$'; then
  echo "invalid task_run_id for branch publication: $task_run_id" >&2
  exit 64
fi

if [ ! -e "$worktree/.git" ]; then
  echo "not a git worktree root (missing .git metadata): $worktree" >&2
  exit 66
fi

if ! git_top=$(git -C "$worktree" rev-parse --show-toplevel 2>/dev/null); then
  echo "not a git worktree root (git rev-parse failed): $worktree" >&2
  exit 66
fi

worktree_root=$(cd "$worktree" && pwd -P)
git_root=$(cd "$git_top" && pwd -P)
if [ "$git_root" != "$worktree_root" ]; then
  echo "not a git worktree root: $worktree resolves to git top-level $git_root" >&2
  exit 66
fi

if [ -n "$base_commit" ]; then
  if ! git -C "$worktree" cat-file -e "$base_commit^{commit}" 2>/dev/null; then
    echo "invalid base commit: $base_commit" >&2
    exit 65
  fi
  if ! git -C "$worktree" merge-base --is-ancestor "$base_commit" HEAD; then
    echo "base commit is not an ancestor of HEAD: $base_commit" >&2
    exit 65
  fi
fi

branch="agentd/$task_run_id"

git -C "$worktree" switch -C "$branch" >&2
git -C "$worktree" add -A >&2

if ! git -C "$worktree" diff --cached --quiet; then
  git -C "$worktree" commit -m "agentd $task_run_id" >&2
elif [ -n "$base_commit" ]; then
  if git -C "$worktree" diff --quiet "$base_commit" HEAD --; then
    echo "refusing publication: no task delta relative to $base_commit" >&2
    exit 65
  else
    diff_status=$?
    if [ "$diff_status" -ne 1 ]; then
      echo "failed to compare publication with base commit: $base_commit" >&2
      exit "$diff_status"
    fi
  fi
fi

git -C "$worktree" push -u origin "HEAD:$branch" >&2

mkdir -p "$(dirname "$report")"
{
  printf '%s\n\n' '# agentd acceptance report'
  printf '%s\n' "- task_run_id: $task_run_id"
  printf '%s\n' "- branch: $branch"
  printf '%s\n' "- worktree: $worktree"
  if [ -n "$base_commit" ]; then
    printf '%s\n' "- base_commit: $base_commit"
  fi
  printf '%s\n' "- head_commit: $(git -C "$worktree" rev-parse HEAD)"
  printf '%s\n' "- status: published"
} >"$report"

printf '%s\n' "$branch"
