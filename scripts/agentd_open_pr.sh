#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -gt 2 ] || [ "$#" -lt 1 ]; then
  echo "usage: $0 <task_run_id> [base_branch]" >&2
  exit 64
fi

task_run_id=$1
base_branch=${2:-main}
remote=origin

if ! printf '%s' "$task_run_id" | grep -Eq '^tr_[0-9A-HJKMNP-TV-Z]{26}$'; then
  echo "invalid task_run_id for PR creation: $task_run_id" >&2
  exit 64
fi

if ! printf '%s' "$base_branch" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._/-]*$'; then
  echo "invalid base branch for PR creation: $base_branch" >&2
  exit 64
fi

branch="agentd/$task_run_id"
base_ref="$remote/$base_branch"

print_no_common_history_repair() {
  {
    echo "repair task branch with:"
    echo "  git switch $branch"
    echo "  bash scripts/agentd_pr_history_bridge.sh $base_branch"
    echo "  AGENTD_PR_HISTORY_BRIDGE=1 bash scripts/agentd_pr_history_bridge.sh --execute $base_branch"
    echo "  git push $remote $branch"
    echo "then retry:"
    echo "  bash scripts/agentd_open_pr.sh $task_run_id $base_branch"
  } >&2
}

git fetch "$remote" "+refs/heads/$base_branch:refs/remotes/$remote/$base_branch" >&2

if ! git rev-parse --verify --quiet "$branch^{commit}" >/dev/null; then
  echo "published task branch not found locally: $branch" >&2
  exit 66
fi

if ! git ls-remote --exit-code --heads "$remote" "$branch" >/dev/null 2>&1; then
  echo "published task branch not found on $remote: $branch" >&2
  exit 66
fi

if ! git rev-parse --verify --quiet "$base_ref^{commit}" >/dev/null; then
  echo "base branch not found after fetch: $base_ref" >&2
  exit 66
fi

if ! git merge-base "$base_ref" "$branch" >/dev/null; then
  echo "cannot open PR: $branch has no common history with $base_ref" >&2
  print_no_common_history_repair
  exit 65
fi

gh pr create --fill --base "$base_branch" --head "$branch"
