spec: task
name: "AD-E5 daemon native artifact content-store acknowledgement"
tags: [e2e, native-runtime, artifact, lease, redaction]
---

## Intent

Make the daemon native execution boundary persist bounded runtime output in a
content-addressed store and acknowledge the resulting immutable artifact under
the exact current lease and fencing token. Failed acknowledgement must leave a
retryable durable spool record without rerunning the native process.

## Decisions

- Native output is read from the bounded runtime snapshot and passed through
  `LocalContentStore` before an artifact envelope is built.
- The artifact `storage_ref` is a stable `local://<sha256>` reference; raw host
  spool paths are not published as enterprise storage references.
- Artifact acknowledgement is admitted by the current `TaskLeaseClaim`; stale,
  expired, or mismatched fencing claims are rejected before publication.
- Redaction occurs before hashing and storage; empty secret values are ignored.
- Existing file-spool APIs remain compatibility paths and are not removed in
  this slice.

## Boundaries

### Allowed Changes

- crates/agentd-bin/src/native_worker.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/tests/**
- crates/agentd-store/src/content_store.rs
- crates/agentd-store/src/lib.rs
- crates/agentd-store/tests/**
- crates/agentd-core/src/ports/execution_evidence.rs
- specs/e2e/ad-e5-daemon-native-artifact-ack.spec.md

### Forbidden

- Do not execute Claude, tmux, Matrix, Robrix, or remote services in tests.
- Do not publish an artifact before lease/fencing admission.
- Do not store raw secret material or raw host spool paths in enterprise
  artifact metadata.
- Do not rerun a native process when acknowledgement fails.
- Do not weaken existing standalone compatibility behavior.

## Completion Criteria

Rule: content-addressed-native-evidence

Scenario: native output is content addressed before acknowledgement
  Test:
    Package: agentd-store
    Filter: content_store::tests::put_and_get_are_content_addressed
  Given bounded native output bytes
  When the daemon stores the output
  Then the returned reference is `local://<sha256>` and the bytes can be read
  And the recorded size equals the stored byte length

Scenario: corrupted content is rejected
  Test:
    Package: agentd-store
    Filter: content_store::tests::corrupted_object_is_rejected
  Given a stored content object whose bytes are changed afterward
  When the object is read by digest
  Then the content store returns a hash mismatch

Scenario: redaction happens before content addressing
  Test:
    Package: agentd-bin
    Filter: tests::redaction_replaces_every_occurrence_in_binary_output
  Given output containing the same non-empty secret more than once
  When the native worker redacts output
  Then every occurrence is replaced before content addressing
  And an empty secret does not modify the output

Scenario: acknowledgement can be retried without rerunning
  Test:
    Package: agentd-bin
    Filter: native_worker
  Given a persisted spool record and an acknowledgement failure
  When the caller retries with the same envelope
  Then the runtime process is not started again
  And the same content digest and storage reference are reused

Scenario: compatibility file spool remains available
  Test:
    Package: agentd-tmux
    Filter: native
  Given the existing file-spool compatibility API
  When standalone code requests a file spool
  Then it still writes the bounded output to the requested path
  And enterprise storage references are only produced by the content-store API

Scenario: stale fencing is rejected before artifact publication
  Test:
    Package: agentd-core
    Filter: capability_admission::capability_rejects_stale_fencing_token
  Given an artifact admission whose claim fencing token differs from the
  capability token
  When the artifact boundary validates the admission
      Then it returns `LeaseRejected` before any protected side effect

Scenario: production pull execution uses the configured object store and evidence port
  Test:
    Package: agentd-bin
    Filter: recovery_http
  Given a worker service configured with a content store and the durable evidence port
  When a validated native pull grant reaches execution
  Then the native output is stored, published, and acknowledged before lease release
  And an acknowledgement failure cancels the lease without starting a second process

## Out of Scope

- Cloud-provider-specific signing and multipart orchestration beyond the
  S3-compatible adapter.
- Matrix/Robrix/dashboard presentation changes.
- Full worker wire protocol and process supervision.
- Real native agent or Claude/Codex smoke execution.
