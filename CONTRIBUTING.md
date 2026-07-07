# Contributing to agentd

## Before opening a PR

Run the local quality gate from a clean checkout:

```bash
./scripts/check.sh
```

This mirrors CI. If it passes locally and you haven't introduced a flaky test, CI will pass.

## Commit Conventions

- Conventional commits: `feat:` / `fix:` / `docs:` / `test:` / `refactor:` / `chore:` / `ci:` / `spec:`
- Scope = crate or area: `feat(core): ...`, `test(tmux): ...`
- Body references the spec when applicable: `Refs: specs/tmux/p11-prompt-injection-buffer-path.spec.md`

## TDD Discipline

For every new behavior:

1. Write the failing test (cite the spec scenario name).
2. Run the test → confirm failure with the expected message.
3. Implement the minimal code.
4. Run the test → confirm pass.
5. `cargo clippy` clean.
6. Commit.

Refactors get the same loop; if no test exists, add a characterization test first.

## Forbidden Patterns

The CI gate enforces these. Don't try to work around them — fix the root cause.

- `unwrap()` outside tests (clippy `unwrap_used`, opted-in per production crate)
- `send-keys -l <payload>` anywhere in `crates/*/src/**` (use the buffer path — design §4.6)
- References to `palace.db` in `crates/*/src/**` (mempal's DB, off-limits — design §3.1)
- `tokio::time::sleep` for test timing (use `tokio::time::pause` + `advance`)
- Real tmux / Matrix / mempal inside `--lib` or unit/integration tests (those belong in e2e)
- `#[ignore]` without a `TODO #N` comment and ≤ 30-day expiry

## Spec-Driven Workflow

Each feature has a `.spec.md` contract under `specs/<area>/`. The test
function name must match the spec's `Test:` selector verbatim, or
`agent-spec guard` fails CI with a "dangling selector".

- `agent-spec lint <spec>` — check one contract's quality (≥ 0.7).
- `agent-spec lifecycle <spec> --code . --format text` — lint + run the
  spec's bound tests (takes ONE spec file, not a glob; `--format` is
  `text|json|md`).
- `agent-spec guard --code .` — repo-wide: lint all specs + verify against
  the git change scope.

Authoring: read `~/.claude/skills/agent-spec-authoring/` or run
`agent-spec init --level task --lang en --name "..."`.

## Storage

agentd uses sqlx in "runtime" mode — no compile-time query checking (no
`.sqlx/` metadata committed). Use `query_as::<_, RowStruct>(...)` for typed
results. We may switch to `query!` macros in a later phase once the schema
is stable.
