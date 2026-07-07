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
- `codergen`: `run` assembles the initial prompt from the context vars named in `initial_prompt_includes` (`$var` → context key) plus best-effort `pre_tools` mempal results, spawns the agent, records a `task_runs` row, stages `task_run_id` (ctx-staged so the pre-park checkpoint captures it), and parks `AgentOutcome { task_run_id }`. `resume(AgentOutcomeSubmitted)` **closes the task run** (`complete_task_run`, so a replayed event is a no-op — mirrors `wait.human`) and returns the agent-reported `Outcome` verbatim — no blocking wait.
- `fan_out`: `run` derives N from the `reviewers` comma-list, computes `context_sha = sha256(serialize(context) ++ node_id)` in memory (D7 — no disk bundle), records a `review_runs` row, spawns N reviewer agents, stages `review_run_id` (so the paired `fan_in` can read it), and parks `ReviewVerdicts { review_run_id, expected: N }`. P111 adds optional `stance_queries` and `prompt_profiles` reviewer maps; when present, each reviewer gets only its own best-effort mempal stance pack and prompt profile while sharing the same `context_sha`. `resume(ReviewVerdictSubmitted)` records the verdict and opaque findings (idempotent per reviewer, so a duplicate cannot reach quorum early) and re-parks until `count_verdicts == expected`, reading the **authoritative `expected` from the stored review run** (not re-derived from the live node, which may differ across an `--accept-workflow-change` resume), then returns `Done`.
- `EngineEvent::ReviewVerdictSubmitted` carries the `VerdictValue` plus opaque `findings` text (extended by P113 — the resume must record the vote and findings, not just the reviewer). `VerdictValue`/`ReviewVerdict` moved to `types` since the Store port, the engine event, and `fan_in` all reference them.
- `fan_in`: reads `review_run_id` from the context, lists verdicts, applies `aggregator` → `Status`: `any_fail` (any Fail/Block ⇒ Fail), `majority_pass` (strict Pass majority), `unanimous_pass` (all Pass), `first_blocker` (any Block ⇒ Fail, plain Fails advisory). P109 adds the `converge_or_<fallback>` family; P110 makes non-final Delphi rounds return `PartialSuccess` with `delphi_next_round` and `delphi_previous_verdicts`, then applies the fallback when verdicts stabilize or max_rounds exhausts. P113 adds `delphi_previous_findings` and `findings_diff<N>` convergence.

## Boundaries

### Allowed Changes

- specs/core/p7-handlers-fan-out-fan-in-wait-human.spec.md
- crates/agentd-core/src/handler/{wait_human,fan_out,fan_in,codergen}.rs and mod.rs/registry.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/engine/step.rs (add VerdictValue to ReviewVerdictSubmitted)
- crates/agentd-core/src/types/{verdict.rs,mod.rs}; ports/store.rs + ports/mod.rs (import moved types)
- crates/agentd-core/src/test_support/mempal_stub.rs
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

Scenario: fan_out gives each reviewer a distinct stance pack
  Test: fan_out_adds_distinct_stance_pack_to_each_reviewer_prompt
  Given a fan_out node with two reviewers and two stance_queries entries
  When the handler runs
  Then each reviewer prompt contains only that reviewer's mempal stance-pack hit

Scenario: fan_out gives each reviewer its prompt profile
  Test: fan_out_adds_per_reviewer_prompt_profiles
  Given a fan_out node with two reviewers and two prompt_profiles entries
  When the handler runs
  Then each reviewer prompt contains only that reviewer's prompt_profile

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

Scenario: fan_in converge_or_majority_pass returns Success on a majority fallback
  Test: fan_in_converge_or_majority_pass_uses_majority_fallback
  Given a final review round with two Pass and one Fail and aggregator converge_or_majority_pass
  When fan_in runs
  Then it returns Done with status Success

Scenario: fan_in converge_or_majority_pass requests the next Delphi round before max_rounds
  Test: fan_in_converge_or_majority_pass_requests_next_round_before_max
  Given a non-final Delphi round with two Pass and one Fail and aggregator converge_or_majority_pass
  When fan_in runs without a previous verdict signature
  Then it returns Done with status PartialSuccess and context delphi_next_round=2

Scenario: fan_in converge_or_majority_pass finishes when Delphi verdicts stabilize
  Test: fan_in_converge_or_majority_pass_finishes_when_verdicts_stabilize
  Given a round 2 Delphi review whose verdict signature matches delphi_previous_verdicts
  When fan_in runs
  Then it returns the majority_pass fallback status without requesting another round

Scenario: fan_in findings_diff requests another Delphi round when findings differ
  Test: fan_in_findings_diff_requests_next_round_when_findings_changed_above_threshold
  Given a round 2 Delphi review whose findings signature differs above the threshold
  When fan_in runs
  Then it returns PartialSuccess with delphi_next_round and delphi_previous_findings

Scenario: fan_in findings_diff finishes when findings converge
  Test: fan_in_findings_diff_finishes_when_findings_change_below_threshold
  Given a round 2 Delphi review whose verdicts changed but findings diff is below threshold
  When fan_in runs
  Then it returns final fallback status instead of requesting another round

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

Scenario: fan_out ignores a duplicate reviewer verdict (replay-safe quorum)
  Test: fan_out_resume_ignores_duplicate_reviewer_verdict
  Given a fan_out node expecting three reviewers that has parked
  When the first reviewer's verdict is replayed before the others vote
  Then the duplicate does not advance quorum and only three distinct reviewers complete it

Scenario: codergen resume closes the task run so a replay is a no-op
  Test: codergen_resume_closes_task_run_so_replay_is_noop
  Given a codergen node that has parked with a task run
  When it is resumed with the agent outcome
  Then lookup_park_by_task_run returns None afterward
