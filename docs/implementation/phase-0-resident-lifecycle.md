# Phase 0 Resident lifecycle

This checkpoint gives the authenticated Operator explicit control over whether an admitted
Resident identity is available for later assignment. It does not make Residents capable of work.

## Closed lifecycle

Residents begin `inactive`. The Operator can activate an inactive Resident, suspend an active
Resident, reactivate a suspended Resident, or permanently retire any non-retired Resident.
Retirement is terminal. Each command carries the latest decision ID observed by its caller (or
`null` before the first decision), so stale screens fail closed.

The command and result shapes contain identity, role, optional Display Name, lifecycle, timestamps,
and the latest decision ID. They contain no model, Beacon Capability, Route, Worker, credential,
Policy, Team, Mission, budget, data scope, network permission, Run, or execution authority. An
`active` Resident is only available to future policy; current Phase 0 still cannot start work.

## Persistence and reconciliation

Schema version 8 widens the `agents.lifecycle` projection and adds the append-only
`resident_lifecycle_decisions` ledger. Each row records the exact prior and next lifecycle,
predecessor, authenticated Operator actor, and decision time. Partial unique indexes prevent two
initial decisions or two successors from the same predecessor, while database triggers reject
updates and deletes.

beacond writes the decision, typed lifecycle Event, and current `agents` projection in one immediate
transaction. Startup refuses to report ready unless every Resident chain:

- starts from the admitted inactive state and follows only legal transitions;
- names the exact prior decision with nondecreasing time;
- contains only Operator decisions and stops permanently at retirement;
- has exactly one byte-equivalent typed Event per decision; and
- ends at the lifecycle, decision ID, and timestamp projected on the Agent row.

The original Starter Organization admission and `starter_organization.accepted` Event continue to
describe the immutable inactive admission snapshot. Current lifecycle is queried separately.

## Interfaces and presentation

The same closed types cross the public Rust service, authenticated Unix socket, Tauri commands,
capability allowlist, and strict TypeScript adapter. Locked sessions cannot read or mutate the
roster. Briefing exposes only actions legal for each state, labels lifecycle as availability rather
than authority, and requires an accessible confirmation step before permanent retirement.

## Verification

From the repository root:

```sh
cargo test -p beacond --test resident_lifecycle
cargo test -p beacond --test local_transport authenticated_client_controls_resident_lifecycle_over_the_real_socket -- --exact
cargo test --workspace
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The research basis and 15-case authenticated-socket measurement are recorded in
[`docs/evals/2026-08-28-resident-lifecycle.md`](../evals/2026-08-28-resident-lifecycle.md).
