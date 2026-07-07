#!/usr/bin/env bash
set -euo pipefail
cd '.agentd/real-execute-smoke/real-execute-smoke-20260707070231/worktrees/wt-task-tr_01KWWTQJP95Z23Y1ZBRKXMS29T'
export AGENTD_MCP_STDIO_CMD=''\''/Users/zhangalex/Work/Projects/AI/agentd/target/debug/agentd'\'' --db-path '\''/Users/zhangalex/Work/Projects/AI/agentd/.agentd/real-execute-smoke/real-execute-smoke-20260707070231/agentd.db'\'' --workflows-dir '\''/Users/zhangalex/Work/Projects/AI/agentd/workflows'\'' --repo-dir '\''/Users/zhangalex/Work/Projects/AI/agentd'\'' --worktree-base '\''/Users/zhangalex/Work/Projects/AI/agentd/.agentd/real-execute-smoke/real-execute-smoke-20260707070231/worktrees'\'' --log-level '\''error'\'' mcp-stdio'
exec claude --mcp-config '.agentd/real-execute-smoke/real-execute-smoke-20260707070231/worktrees/wt-task-tr_01KWWTQJP95Z23Y1ZBRKXMS29T/.agentd-mcp-implementer.json' --strict-mcp-config
