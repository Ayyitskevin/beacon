---
status: accepted
---

# Take encrypted Safety Snapshots before risky transitions

Beacon automatically creates and verifies an encrypted Safety Snapshot before installing an update
and before dispatching a Proposal that Policy classifies as high-impact. The snapshot covers
Beacon-owned Workspace state and the declared mutable targets Beacon can safely restore. If a
required snapshot fails, the transition fails closed.

## Consequences

A Safety Snapshot is a recovery point, not an Export Package, authority grant, or guarantee that
external effects are reversible. Beacon must identify uncaptured or non-restorable effects and keep
them gated. Snapshot scope, integrity, retention, and key reference remain inspectable; secret keys
stay in the operating system's secret store. Restore is an explicit, attributable Operator action,
and update snapshots are taken only after Runs and Worker leases have quiesced. Default retention
keeps the latest ten verified snapshots plus one verified daily snapshot for 30 days under an
Operator-adjustable storage quota; overlapping snapshots are stored once.

The Operator may choose an off-device destination without using a Beacon-hosted service. Beacon
encrypts before transfer, verifies the completed copy, and reports destination failure explicitly.
Policy decides whether the verified local snapshot alone satisfies a pending transition.
