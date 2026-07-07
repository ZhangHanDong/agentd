#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <worktree> <task_run_id>" >&2
  exit 64
fi

worktree=$1
task_run_id=$2

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

branch="agentd/$task_run_id"

git -C "$worktree" switch -C "$branch" >&2
git -C "$worktree" add -A >&2

if ! git -C "$worktree" diff --cached --quiet; then
  git -C "$worktree" commit -m "agentd $task_run_id" >&2
fi

git -C "$worktree" push -u origin "HEAD:$branch" >&2

report=".agentd/run/report.md"
mkdir -p "$(dirname "$report")"
{
  printf '%s\n\n' '# agentd acceptance report'
  printf '%s\n' "- task_run_id: $task_run_id"
  printf '%s\n' "- branch: $branch"
  printf '%s\n' "- worktree: $worktree"
  printf '%s\n' "- status: published"
} >"$report"

printf '%s\n' "$branch"
