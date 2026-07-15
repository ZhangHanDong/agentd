#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <worktree> <base_commit>" >&2
  exit 64
fi

worktree=$1
base_commit=$2

if [ ! -d "$worktree" ] || [ ! -e "$worktree/.git" ]; then
  echo "not a git worktree root: $worktree" >&2
  exit 66
fi

if ! git_top=$(git -C "$worktree" rev-parse --show-toplevel 2>/dev/null); then
  echo "not a git worktree root: $worktree" >&2
  exit 66
fi

worktree_root=$(cd "$worktree" && pwd -P)
git_root=$(cd "$git_top" && pwd -P)
if [ "$git_root" != "$worktree_root" ]; then
  echo "not a git worktree root: $worktree resolves to git top-level $git_root" >&2
  exit 66
fi

if ! git -C "$worktree" cat-file -e "$base_commit^{commit}" 2>/dev/null; then
  echo "invalid base commit: $base_commit" >&2
  exit 65
fi

if ! git -C "$worktree" merge-base --is-ancestor "$base_commit" HEAD; then
  echo "base commit is not an ancestor of HEAD: $base_commit" >&2
  exit 65
fi

if git -C "$worktree" diff --quiet "$base_commit" HEAD --; then
  :
else
  diff_status=$?
  if [ "$diff_status" -eq 1 ]; then
    printf '%s\n' "task delta verified relative to $base_commit"
    exit 0
  fi
  echo "failed to compare task worktree with base commit: $base_commit" >&2
  exit "$diff_status"
fi

if [ -n "$(git -C "$worktree" status --porcelain --untracked-files=all)" ]; then
  printf '%s\n' "task delta verified relative to $base_commit"
  exit 0
fi

echo "no task delta relative to $base_commit" >&2
exit 65
