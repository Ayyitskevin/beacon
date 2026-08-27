---
status: accepted
---

# Use one control plane with leased remote workers

One operator workstation owns Beacon's state, policy, authority, and supervision while local or
remote fleet machines run small Workers. Discovery leaves a Worker inactive until the Operator
activates it and approves its advertised capabilities. Workers execute signed, time-limited Run
leases through the same Execution interface as a local Worker. This preserves one source of truth
and lets Beacon grow into a complete distributed product without turning every node into an
independently authoritative Installation. Separate Installations remain independent and do not
synchronize or federate.

## Consequences

Workers advertise resources but cannot grant authority, create policy, or mutate Project truth.
Leases bind Worker, Proposal, policy, inputs, effects, sandbox, network, time, and budget. A
disconnected Worker receives no new work and can continue only within an unexpired lease. The
control plane verifies its signed Receipt before reporting success. Alpha 1 uses the local Worker
adapter; remote transport follows the proven local contract. Cross-Installation exchange is limited
to explicit future export, import, or Handoff packages until federation receives its own design.
