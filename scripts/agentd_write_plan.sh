#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <frozen-spec> <plan-path>" >&2
  exit 64
fi

spec_path=$1
plan_path=$2

if [ ! -f "$spec_path" ]; then
  echo "frozen spec does not exist: $spec_path" >&2
  exit 66
fi

mkdir -p "$(dirname "$plan_path")"
tmp="${plan_path}.tmp.$$"
trap 'rm -f "$tmp"' EXIT

agent-spec plan "$spec_path" >"$tmp"
mv "$tmp" "$plan_path"
trap - EXIT

printf '%s\n' "$plan_path"
