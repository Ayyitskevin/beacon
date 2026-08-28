# Phase 0 Starter Organization

This checkpoint replaces the empty post-enrollment screen with one authenticated, editable roster
proposal. It creates identities, not a running fleet.

## Contract

Proposal version 1 contains four closed role templates in canonical order:

1. Lead, recommended;
2. Builder, recommended;
3. Reviewer, recommended;
4. Researcher, available but not preselected.

The Owner may change the inclusion set and assign an optional Display Name of 1–80 trimmed,
non-control characters. Acceptance requires at least one unique role. Discard is a separate
decision and the desktop requires a second confirmation. Either decision is recorded once.

Acceptance creates only inactive Resident identities. The command has no fields for a model,
provider, capability, tool, budget, activation, Policy, Gate, or execution authority. The Starter
Organization cannot run work. Those are later domain decisions through their own interfaces.

## Ownership and timing

`StarterOrganizationService` is the Rust command/query boundary. The desktop, Tauri adapter, and
local socket client use its typed request and result objects instead of reading tables. The service
validates edits before touching the store, then rechecks the live authenticated Owner session and
executes the decision, Resident inserts, and typed Event in one immediate transaction.

`beacond` remains the only store writer. A process restart reconciles the decision, the exact
immutable starter-admission snapshots, their Resident identities, and the Event before reporting
the store ready. Later lifecycle or identity presentation changes therefore cannot rewrite the
historical acceptance. A source-specific partial uniqueness constraint allows one starter identity
per proposed role without preventing future teams from containing multiple Residents with the same
role. A missing or duplicate decision Event is a startup failure rather than silently accepted
history.

## Failure behavior

Locked sessions cannot read or decide the proposal. Stale proposal versions, empty acceptance,
duplicate roles, invalid Display Names, and repeated decisions fail closed. Invalid edits create no
decision, Resident, or Event. Concurrent decisions serialize through the account/store boundary;
exactly one commits.

The TypeScript adapter rejects missing, reordered, widened, or malformed response shapes. In
particular, an undeclared `authority` field is not ignored. The Briefing reports every admitted
Resident as inactive and states that no Resident has execution authority.

## Verification

From the repository root:

```sh
cargo test -p beacond --tests
cd apps/desktop && npm test -- --run src/session.test.ts src/App.test.tsx
scripts/gate all
```

The real endpoint cases and observed results are recorded in
[`docs/evals/2026-08-27-starter-organization-endpoint.md`](../evals/2026-08-27-starter-organization-endpoint.md).
