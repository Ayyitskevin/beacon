---
status: accepted
---

# Host multiple isolated Workspaces in one Installation

One Beacon Installation may host several Workspaces so one operator can maintain separate agent
organizations and bodies of work in the same desktop suite. Each Workspace is its own trust and
authority boundary even though beacond and the managed store are shared infrastructure.

## Consequences

Every command, event, identity, policy, budget, credential reference, memory record, Artifact, and
lease belongs to one Workspace. Nothing is visible, spendable, executable, or reusable across
Workspaces by default. Installation-level discovery may suggest the same local resource to several
Workspaces, but the Operator activates and scopes it independently in each one. Updates,
diagnostics, and storage maintenance remain Installation concerns. An Installation may save a
non-secret Resource Preset and propose it in another Workspace, but the target Workspace receives
no activation, secret, capability, budget, or authority until its Operator approves them there.

Selected work crosses this boundary only through an immutable Handoff Package. The destination
Workspace treats it as untrusted input, verifies its provenance, and explicitly adopts new local
records. The package carries no secret, live reference, identity, Policy, budget, or authority, and
creates no continuing link to its source Workspace.
