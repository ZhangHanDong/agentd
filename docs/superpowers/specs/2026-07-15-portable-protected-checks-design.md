# Portable Protected Checks Design

## Problem

PR #21 passed formatting, Clippy, dependency, boundary, and DOT checks, but its
unit and contract jobs exposed three host assumptions: a workstation-specific
agent-chat checkout, an installed `agent-spec` binary inside a shell test, and
full lifecycle execution for an explicitly blocked future contract. A follow-up
macOS run also exposed Bash 3.2 nounset behavior when an empty array is expanded.

## Design

Parity map sources are repository-relative. `agentctl parity audit` accepts a
relative source only when every path component is normal, and accepts an
absolute compatibility source only when it is contained by the canonical
agent-chat root. This makes one map usable with any checkout while rejecting
`..` traversal. Sources are citation strings: parity audit does not open or
execute them. Filesystem dereference and symlink-target containment are
therefore outside this validation boundary; canonicalizing a source would also
reject valid references whose target is not present in a partial checkout.

Shell integration tests own command dependencies through a temporary `PATH`.
The prepare-only smoke supplies a fake planner that supports only
`agent-spec plan`, so it proves rendering without starting a provider.
Matrix service smoke tests use `/bin/bash` on macOS to match the protected
runner. The script checks each repeatable option array's length before expansion,
which is portable to Bash 3.2 with `set -u`.

`scripts/agentd_verify_spec.sh` is the single recursive spec classification
point. It parses JSON metadata with `jq`; explicit `design-only` and
`template-only` contracts receive parse and lint, while every other contract
receives lifecycle. Implemented historical contracts retain lifecycle and point
to their current equivalent integration test names. Smoke rendering removes
`template-only` from the frozen run contract before planning and execution, so
generated work must still satisfy lifecycle.

`scripts/agentd_guard_changed_contract.sh` provides the boundary gate that the
non-recursive `agent-spec guard --code .` command cannot provide for this
repository. It reads the staged delta locally, a single HEAD commit for fallback
jobs, or a base-to-head range in CI. It selects changed implementation contracts
and passes every changed path to lifecycle with explicit `--change` arguments.
At least one changed implementation contract must govern the complete delta.
Design and template contracts stay visible to recursive parse/lint verification
without claiming implementation ownership. NUL-delimited git output preserves
unusual filenames; deletions and type changes remain in the delta, and rename
detection is disabled so both the old and new path are boundary checked.

The existing reconciliation branch predates this guard and contains historical
fixup commits without task contracts. Its first protected run therefore
bootstraps at the commit introducing P156. After P156 exists in the target base,
pull requests are checked across the complete merge-base-to-head range. Push
events use the event's before revision, while scheduled fallback checks inspect
HEAD.

Local and CI gates require exactly `agent-spec 1.0.0`; installation absence or
version drift fails the gate. CI checks out the pull request head with complete
history to identify range boundaries rather than relying on GitHub's synthetic
merge checkout.

## Verification

Targeted integration tests cover portable parity auditing, traversal rejection,
isolated smoke planning, gate wiring, changed-path propagation, and lifecycle
failure propagation. They also exercise empty repeatable Matrix arguments under
the macOS system Bash. The P156 lifecycle, changed-contract guard, complete spec
audit, workspace tests, formatting, Clippy, and dependency governance remain
required before push.
