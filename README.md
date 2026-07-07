# agentd

A Rust daemon that orchestrates multi-agent software workflows. Drop-in successor to [agent-chat](../../consult/agent-chat) with a typed workflow engine, formal review semantics, and clean integration with [mempal](../mempal), [agent-spec](../../FW/rust-agents/agent-spec), and Robrix.

**Status**: P0.0 complete (workspace + CI + scaffold). Design: [`docs/specs/2026-05-29-agentd-design.md`](docs/specs/2026-05-29-agentd-design.md). **Role boundary** (authoritative where it conflicts with the design doc): [`docs/specs/2026-05-29-agentd-specify-boundary.md`](docs/specs/2026-05-29-agentd-specify-boundary.md) — agentd is the **local execution runtime**; the web project-context / spec-collaboration layer (**Specify**) is a separate project on top. The "What it does" sketch below predates that boundary and is reframed in a later doc pass.

## What it does

```
  GitHub issue ──────────────────────► .agentflow/issue-to-pr.dot ──────────────► PR
                                                  │
                                                  ▼
   spec-writer → wait.human approve → planner → implementer → adversarial review (N agents) → aggregate
                                          │
                                          └── stance pack per reviewer from mempal_search
```

Coordinated through a per-project Matrix room (Robrix is the primary client).

## What it isn't

Not a memory system (use mempal). Not a spec DSL (use agent-spec). Not an IM protocol (use Matrix). Not an LLM SDK (claude-code / codex CLIs already cover this). Not a TUI (octos-tui exists). See design doc §7.5.

## Layout

```
crates/
├── agentd-core/        # workflow engine, traits, domain types (no I/O)
├── agentd-tmux/        # TmuxBackend impl
├── agentd-store/       # sqlx + sqlite
├── agentd-mempal/      # rmcp client wrapper
├── agentd-specify/     # optional Specify client/adapter seam
├── agentd-github/      # octocrab + webhook
├── agentd-matrix/      # matrix-sdk + cowork-bus gateway
├── agentd-surface/     # axum HTTP+SSE + rmcp server
└── agentd-bin/         # main()
crates/agentctl/        # CLI client
workflows/              # shipped .dot templates
specs/                  # agent-spec contracts for agentd itself
docs/specs/             # design + spec docs
```

## Building

Requires Rust stable (MSRV 1.85), tmux 3.3+, and the `agent-spec` CLI:

```bash
cargo build --workspace
```

Optional but recommended local tooling:

```bash
cargo install --locked cargo-nextest cargo-deny agent-spec
```

## Testing

Run the full local quality gate (mirrors CI):

```bash
./scripts/check.sh
```

Just the Rust tests:

```bash
cargo nextest run --workspace
```

Specific crate:

```bash
cargo nextest run -p agentd-core
```

## License

TBD.
