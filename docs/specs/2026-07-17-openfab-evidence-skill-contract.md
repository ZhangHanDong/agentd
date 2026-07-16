# OpenFab Evidence and Skill Contract

- Status: AD-E4 code candidate
- Schema: execution evidence v1, OpenFab certification v1
- Owners: agentd executes and records evidence; OpenFab certifies and publishes Skill Hub trust

## Authority Boundary

Agentd may sign only execution evidence using a registered Builder or Worker key.
It must never create an OpenFab certification verdict, certification signature,
or Skill Hub trust signature. OpenFab results enter agentd only through
`CertificationPort` and must pass the configured OpenFab key trust policy before
they can be recorded or used.

Certification failure is non-blocking when the execution snapshot uses
`gate=none`. A machine gate requires a passing machine attestation. A human gate
requires the exact snapshot N-of-M threshold in addition to the passing machine
attestation. Release and merge admission always rebind the exact snapshot,
source commit, artifact subject digest, certification policy version and digest,
and stored OpenFab result.

## Signed Bytes

Signatures cover UTF-8 canonical JSON of the payload. Canonical JSON uses compact
JSON, preserves array order, and recursively sorts object keys lexicographically.
`payload_sha256` is lowercase SHA-256 of those exact bytes. Signatures are
Ed25519 encoded with standard padded base64. Public keys use Ed25519
`did:key:z...` with multicodec prefix `0xed01`, matching OpenFab identity
encoding.

The `skill_packages_sha256` field is lowercase SHA-256 of canonical JSON for the
ordered `SkillPackageEvidenceRef` array. A certification result must carry the
same digest as its request.

## Key Lifecycle

Trusted keys have one role, DID, validity window, optional revocation time, and
optional successor. Key identity and lifecycle history are immutable. A
signature created before revocation remains verifiable; a signature at or after
revocation is rejected. Register the successor before marking the old key as
superseded.

## Durable Protocol

Migration `0020_openfab_evidence_skill.sql` stores:

- trusted public-key lifecycle history;
- immutable signed execution envelopes;
- immutable certification requests and independently delivered request events;
- immutable externally signed results and independently recorded result events;
- append-only delivery/certification/release/revocation transitions;
- immutable forge admissions;
- signed Skill Hub trust observations and package installation history.

Every replay is exact. Reusing an envelope id, request id/idempotency key, result
id/request id, forge idempotency key, trust digest, or installation identity with
different bytes is a conflict.

## Skill Hub

Execution snapshots pin package authority, package id, package version, archive,
manifest, dependency lock, and permissions digests. Evidence and certification
carry the same package refs. The HTTPS adapter resolves the exact typed package
version; mutable `latest` references are invalid.

Only a signed, unexpired `approved` or `signed` current trust record permits a
new installation. `draft`, `in_review`, `yanked`, `revoked`, and `deprecated`
records deny new installs. Agentd preserves the signed trust record observed at
installation, so later yanking or revocation does not erase historical
verification.

## Transport

`HttpOpenFabCertificationTransport` uses bearer-authenticated HTTPS, disables
redirects, applies a caller-supplied timeout, bounds result/trust bodies to 1 MiB,
and redacts credentials from debug output. Plain HTTP is allowed only for an
explicit loopback development endpoint.

- `POST v1/certifications/requests`
- `GET v1/certifications/requests/{request_id}/result`
- `GET v1/skills/packages/{authority}/{package}/versions/{version}/trust`

Path values are encoded as URL segments. Request submission carries
`Idempotency-Key`; polling and Skill Hub responses must return the exact requested
typed identity.

## Deferred Acceptance

Code compilation is development feedback, not AD-E4 acceptance. FSF-5 remains
open until the final manual checklist proves independent OpenFab verification,
outage/replay behavior, real key rotation/revocation, all gate modes, Forge
blocking, and Skill Hub yank/revoke behavior against deployed services.
