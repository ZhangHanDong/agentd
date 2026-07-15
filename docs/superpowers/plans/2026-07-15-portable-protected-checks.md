# Portable Protected Checks Implementation Plan

1. Replace the parity audit test's workstation checkout with a temporary
   agent-chat fixture and make capability sources checkout-relative.
2. Reject relative source traversal while retaining contained absolute-source
   compatibility.
3. Give the prepare-only smoke test an isolated fake `agent-spec` planner.
4. Add a shared verifier for explicit design-only or template-only parse/lint
   versus normal lifecycle, and call it from local and GitHub gates.
5. Mark blocked AD-E1/P272 contracts and the unrendered smoke template honestly,
   strip the template marker during rendering, and update stale selectors on
   implemented historical contracts.
6. Add a staged/HEAD/range changed-contract guard that passes every changed path
   to a changed implementation contract through explicit `--change`
   arguments, including deleted, type-changed, and both sides of renamed paths.
7. Add base-to-head range verification with a one-time P156 adoption bootstrap,
   then use the full PR range and push event range for subsequent checks.
8. Pin agent-spec 1.0.0 in local and GitHub gates, strengthen P130 ordering
   coverage, and reject parent components in absolute parity sources.
9. Run Matrix service smoke tests with the macOS system Bash and guard empty
   repeatable option arrays before expansion under Bash 3.2 nounset semantics.
10. Run targeted tests, P156 lifecycle, changed-contract guard, the complete spec
   audit, and full workspace gates; push and monitor PR #21 without merging it.
