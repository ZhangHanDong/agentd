spec: task
name: "Park-style handlers: wait.human, fan_out, fan_in, codergen"
tags: [core, mvp, p0, handlers]
---

## Intent

The four handlers that involve external actors. Three are *park* handlers (D1):
they kick off work, return `HandlerStep::Park`, and unpark via `resume` when the
matching `EngineEvent` arrives — they never block a thread. `fan_in` is
synchronous because `fan_out` already gated on all N verdicts. Together with
Task 7's conditional/tool this completes the six P0 handlers.

## Decisions

- `wait.human`: `run` opens a human-wait row (`Store::open_human_wait`) and parks `HumanAnswer { wait_id }`. `resume(HumanAnswered)` closes the wait (`answer_human_wait`, so a replay is a no-op), stages `answer` (+ `human_feedback`) into the context, and returns `Done` with `preferred_label = answer`.
- `codergen`: `run` assembles the initial prompt from the context vars named in `initial_prompt_includes` (`$var` → context key) plus best-effort `pre_tools` mempal results, spawns the agent, records a `task_runs` row, stages `task_run_id` (ctx-staged so the pre-park checkpoint captures it), and parks `AgentOutcome { task_run_id }`. `resume(AgentOutcomeSubmitted)` returns the agent-reported `Outcome` verbatim — no blocking wait.
- `fan_out`: `run` derives N from the `reviewers` comma-list, computes `context_sha = sha256(serialize(context) ++ node_id)` in memory (D7 — no disk bundle), records a `review_runs` row, spawns N reviewer agents, stages `review_run_id` (so the paired `fan_in` can read it), and parks `ReviewVerdicts { review_run_id, expected: N }`. `resume(ReviewVerdictSubmitted)` records the verdict and re-parks until `count_verdicts == N`, then returns `Done`.
- `EngineEvent::ReviewVerdictSubmitted` carries the `VerdictValue` (extended in this task — the resume must record the vote, not just the reviewer). `VerdictValue`/`ReviewVerdict` moved to `types` since the Store port, the engine event, and `fan_in` all reference them.
- `fan_in`: reads `review_run_id` from the context, lists verdicts, applies `aggregator` → `Status`: `any_fail` (any Fail/Block ⇒ Fail), `majority_pass` (strict Pass majority), `unanimous_pass` (all Pass), `first_blocker` (any Block ⇒ Fail, plain Fails advisory). Delphi/`converge_or_*` are P1+.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/handler/{wait_human,fan_out,fan_in,codergen}.rs and mod.rs/registry.rs
- crates/agentd-core/src/engine/step.rs (add VerdictValue to ReviewVerdictSubmitted)
- crates/agentd-core/src/types/{verdict.rs,mod.rs}; ports/store.rs + ports/mod.rs (import moved types)
- crates/agentd-core/tests/handlers_park.rs

### Forbidden

- Do not block in any of these handlers — `wait.human`/`fan_out`/`codergen` MUST return `Park`; no synchronous `wait_for_outcome` (D1/D7).
- Do not freeze a review bundle to disk in P0.1 — the `context_sha` is computed in memory (D7).

## Completion Criteria

Scenario: wait.human parks awaiting a human answer
  Test: wait_human_run_parks_with_human_answer_reason
  Given a wait.human node
  When the handler runs
  Then it returns Park(HumanAnswer) and an open human wait exists in the store

Scenario: wait.human resume stages the answer and completes
  Test: wait_human_resume_stages_answer_and_returns_done
  Given a wait.human node that has parked
  When resumed with HumanAnswered answer="approve"
  Then it returns Done and the context has answer="approve" staged

Scenario: wait.human resume sets preferred_label to the answer
  Test: wait_human_resume_sets_preferred_label_to_answer
  Given a wait.human node that has parked
  When resumed with HumanAnswered answer="approve"
  Then the returned outcome's preferred_label is "approve"

Scenario: fan_out parks with the expected reviewer count
  Test: fan_out_run_parks_with_expected_reviewer_count
  Given a fan_out node with three reviewers
  When the handler runs
  Then it returns Park(ReviewVerdicts expected=3) and three reviewer agents were spawned

Scenario: fan_out computes a deterministic in-memory context_sha
  Test: fan_out_computes_deterministic_context_sha_in_memory
  Given the same context and node run through fan_out twice
  When each run stages its context_sha
  Then the two context_sha values are equal and 64 hex chars

Scenario: fan_out stays parked until all verdicts arrive
  Test: fan_out_resume_stays_parked_until_all_verdicts_in
  Given a fan_out node expecting three verdicts
  When resumed with the first two verdicts then the third
  Then the first two resumes re-park and the third returns Done

Scenario: fan_in majority_pass returns Success on a majority
  Test: fan_in_aggregator_majority_pass_returns_success_when_majority
  Given a review run with two Pass and one Fail and aggregator majority_pass
  When fan_in runs
  Then it returns Done with status Success

Scenario: fan_in any_fail returns Fail when one reviewer blocks
  Test: fan_in_aggregator_any_fail_returns_fail_when_one_blocker
  Given a review run with two Pass and one Block and aggregator any_fail
  When fan_in runs
  Then it returns Done with status Fail

Scenario: codergen parks awaiting an agent outcome and assembles the prompt
  Test: codergen_run_parks_with_agent_outcome_reason_and_assembles_prompt
  Given a codergen node with initial_prompt_includes and a pre_tools mempal search
  When the handler runs
  Then it returns Park(AgentOutcome), spawns one agent, and the prompt carries the context var and the mempal hit

Scenario: codergen resume returns the agent-reported outcome
  Test: codergen_resume_returns_agent_reported_outcome
  Given a codergen node that has parked
  When resumed with AgentOutcomeSubmitted carrying a Fail outcome
  Then it returns Done with status Fail
