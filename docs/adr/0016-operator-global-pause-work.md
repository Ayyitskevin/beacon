---
status: accepted
---

# Give the Operator one global Pause Work control

Pause Work is an Installation-wide, Operator-only state. Beacon records it before refusing new
Proposal dispatches and new or renewed Worker leases, then requests bounded interruption of active
Runs. This favors truthful, coordinated stopping over an unsafe process kill that could hide
partial external effects.

## Consequences

An active Run remains stopping or unconfirmed until a terminal Receipt or lease expiry proves its
state. Pause Work does not undo effects, revoke Rulings, or stop the supervisor. Only the Operator
may resume work; Agents, Plugins, Workers, schedules, and reconnects cannot. Before Resume Work,
the Operator must review every stopping or unconfirmed Run and either resolve it or explicitly
accept its residual uncertainty.

Pause Work remains available from the open desktop, the system tray, and a configurable global
shortcut when the main window is closed. Beacon supplies a platform-appropriate default that the
Operator may change or disable. Every entry point invokes the same Operator-only Command; none
bypasses Policy, attribution, or the durable paused state.
