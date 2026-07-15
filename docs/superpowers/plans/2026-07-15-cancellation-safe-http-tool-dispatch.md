# Cancellation-safe HTTP tool dispatch implementation plan

1. Add the P154 task contract and record the r2 cancellation evidence.
2. Add a production-host HTTP integration test that cancels a request while a
   post-outcome tool node is blocked; confirm the test fails.
3. Move `/tools/call` dispatch into a daemon-owned Tokio task while preserving
   connected-client responses.
4. Run the P154 selector, lifecycle gate, formatting, clippy, and workspace
   tests.
5. Re-run the P153 Codex-only real execute smoke from a fresh run id.
