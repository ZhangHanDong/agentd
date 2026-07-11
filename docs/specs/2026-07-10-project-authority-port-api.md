# Project Authority Port API and Adapter Design

- **Date**: 2026-07-10
- **Status**: P269 implementation design
- **Authority basis**: `2026-07-10-enterprise-execution-ownership-boundary.md`
- **Reference basis**: `2026-07-10-enterprise-project-room-repo-reference-contract.md`
- **Scope**: resolve, refresh, health, pin, and bounded recovery behavior

## 1. Purpose

P269 turns the P266 reference contract into a callable control-plane boundary.
It does not make agentd a project authority. A selected authority adapter
returns immutable execution snapshots; the control plane validates and pins
those snapshots before execution or recovery.

The same domain values and `ProjectAuthorityPort` calls are used in standalone
and Specify-backed deployments. Adapter selection is explicit. A configured
Specify adapter never falls back to local state when its transport is
unavailable or returns unverifiable data.

## 2. Crate Boundary

`agentd-core` owns pure domain values and the port:

- `AuthorityKey`, closed `ResourceKind`, and typed authority references;
- repository and room binding values;
- `ProjectExecutionSnapshot` plus deterministic validation;
- resolve request, health result, typed authority errors;
- `ProjectAuthorityPort::{resolve, refresh, health}`.

`agentd-project-authority` owns adapters and orchestration:

- `LocalProjectAuthority`, constructed from explicit immutable snapshots;
- `SpecifyProjectAuthority<T>`, wrapping an injected
  `SpecifyAuthorityTransport` without defining an HTTP wire format;
- `ProjectAuthorityControlPlane<P>`, which authorizes a new execution or an
  existing-run recovery against a selected port.

No project authority tables, migration, daemon route, HTTP client, cache, or
background refresh job is added in P269.

## 3. Reference Model

Every authority reference contains exactly authority key, closed resource
kind, opaque resource id, and immutable resource version. Typed wrappers expose
the P266 names and reject the wrong resource kind at construction or snapshot
validation. Equality includes all four tuple fields.

The snapshot carries every P266 field: organization, ordered teams, project,
one or more repository bindings, zero or more room bindings, optional issue,
ordered requirements, frozen spec, product workflow, RBAC, quota,
certification policy, issuance/expiry, canonical content SHA-256, authority
revision, and offline recovery policy.

Repository validation requires exactly one `target` binding and a 40-character
lowercase hexadecimal base commit on that binding. All authority-owned refs
must use the snapshot authority key. Matrix room refs may use their transport
authority key but must have kind `matrix_room`. Content hashes are 64-character
lowercase hexadecimal strings.

## 4. Port Calls

`resolve(request)` selects and validates the current immutable snapshot for a
new execution request. The request names the expected authority and project;
an optional requested snapshot ref allows an exact selection but never a
string value such as `latest`.

`refresh(snapshot_ref)` fetches the exact immutable snapshot previously
pinned. Returning a different snapshot ref, authority, or content is
unverifiable and fails closed.

`health()` reports adapter mode, authority key, availability, observation
time, and optional authority revision. Health is diagnostic only and cannot
authorize execution.

## 5. Adapter Semantics

`LocalProjectAuthority` is available only when explicitly constructed and
selected. Its snapshots are supplied by the standalone composition root. It
resolves the one current snapshot configured for a project and refreshes exact
snapshot refs. Duplicate current snapshots for one project are rejected.

`SpecifyProjectAuthority<T>` delegates the three port calls to an injected
transport. It validates every successful envelope against its configured
authority key and the request. Transport unavailable, malformed, mismatched,
or unverifiable outcomes remain typed fail-closed results. The adapter contains
no local authority and therefore has no fallback path.

## 6. Control-Plane Decisions

For a new execution, `ProjectAuthorityControlPlane` calls `resolve`, validates
the snapshot and current time, verifies the expected project, and returns a
`PinnedProjectSnapshot`. The pinned value is immutable and contains the exact
snapshot plus target repository/base commit derived during validation.

For existing-run recovery, the control plane first calls `refresh` for the
exact pinned snapshot:

- a matching live snapshot authorizes `LiveRevalidated`;
- a changed, expired, revoked, or unverifiable snapshot denies recovery;
- authority unavailability authorizes `OfflinePinned` only when the pinned
  policy is `allow_pinned_until_expiry`, current time is before `valid_until`,
  and the caller presents the unchanged snapshot, target repository, and base
  commit;
- the default `deny` policy and all new-execution failures deny.

Offline recovery does not resolve another project, create another run, change
inputs, deliver source, request certification, or advance workflow authority.

## 7. Verification

Tests use in-memory values and fake transports only. They prove typed reference
separation, complete snapshot validation, local adapter behavior, Specify
forwarding and envelope rejection, absence of fallback, new-run pinning, and
bounded existing-run recovery. No external authority, agent runtime, tmux,
Matrix, Claude, or Codex process is started.
