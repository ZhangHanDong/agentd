spec: task
name: "AD-E3 production artifact object store"
tags: [e2e, artifacts, object-store, s3, minio]
---

## Intent

The native runtime must publish execution bytes through a content-addressed
object-store boundary that can use local storage in development and an
S3/MinIO-compatible HTTP endpoint in production.

## Decisions

- Object keys are the lowercase SHA-256 digest and references use `s3://` or
  `local://`; host filesystem paths are never accepted as artifact references.
- PUT and GET failures are fail-closed; a non-success response cannot produce
  an artifact acknowledgement.
- Retrieved bytes are rehashed before being returned, so corrupt remote data is
  rejected before evidence or acknowledgement consumes it.
- Authorization is supplied by the adapter and is never persisted in an
  artifact record.

## Boundaries

### Allowed Changes

- Cargo.toml
- crates/agentd-store/Cargo.toml
- crates/agentd-store/src/content_store.rs
- specs/e2e/ad-e3-production-object-store.spec.md

### Forbidden

- Do not execute Claude, tmux, Matrix, Robrix, or remote services in tests.
- Do not accept host filesystem paths as remote artifact references.
- Do not acknowledge bytes whose digest has not been verified.

## Completion Criteria

Rule: production-object-store-contract

Scenario: authenticated S3-compatible round trip preserves content address
  Test:
    Package: agentd-store
    Filter: s3_compatible_adapter_puts_gets_and_verifies_content
  Level: adapter contract
  Test Double: wiremock HTTP endpoint
  Given a configured object endpoint and bearer credential
  When artifact bytes are PUT and then GET by their SHA-256 reference
  Then the requests contain the credential and the returned bytes match

Scenario: corrupt remote bytes are rejected
  Test:
    Package: agentd-store
    Filter: s3_compatible_adapter_rejects_corrupt_response
  Level: adapter contract
  Test Double: wiremock HTTP endpoint
  Given a successful GET whose bytes have the wrong digest
  When the adapter resolves the `s3://` reference
  Then it returns a hash mismatch and no artifact bytes

Scenario: invalid references fail before remote access
  Test:
    Package: agentd-store
    Filter: storage_reference_rejects_host_paths_and_unknown_schemes
  Level: adapter contract
  Test Double: local store
  Given a host path or unknown storage scheme
  When the reference is resolved
  Then the operation returns an invalid-reference error

## Acceptance Criteria

### S3-compatible PUT/GET

Given a configured endpoint and bearer credential, when artifact bytes are
stored and retrieved, the adapter sends authenticated requests to the digest
key and returns the same bytes with an `s3://<sha256>` reference.

### Corrupt response rejection

Given a successful GET whose bytes do not match the requested digest, when the
adapter resolves the reference, it returns a hash mismatch and no bytes.

### Missing object

Given a 404 response, when the adapter resolves the reference, it returns
`None` without fabricating an artifact.

### Invalid reference rejection

Given a path traversal, host path, malformed digest, or unknown scheme, when a
reference is resolved, the adapter rejects it before an HTTP request.
