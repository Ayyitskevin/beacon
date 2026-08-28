---
status: accepted
---

# Preserve Project Charters as immutable revisions

Each Project begins with an Operator-authored Charter Revision. Beacon preserves every revision
immutably and records which exact revision is current rather than overwriting Charter text in
place. Project creation atomically writes revision 1, an immutable creation snapshot, and one
attributable `project.created` Event through beacond's authenticated command boundary.

## Consequences

Future Charter changes append a new Operator-authored revision and move the Project's current
revision pointer in the same accepted Command; Residents may never perform that change. Every
Agent-created Mission, alignment decision, Proposal, Ruling, and Receipt names the exact Charter
Revision under which it was evaluated, so a later edit cannot silently broaden earlier authority.
The creation Event reconciles against the immutable creation snapshot and revision 1 rather than
mutable Project fields.

Alpha Project creation accepts only Observe or Supervised autonomy. It creates a no-spend budget
boundary, prohibits external effects, starts no Mission or Run, invokes no model, and activates no
Route, Worker, Plugin, credential, or capability. Bounded autonomy remains unavailable until its
separate safety acknowledgement and Policy requirements exist. This conservative initial envelope
is a real persisted authority boundary, not UI copy or an implicit default that callers can widen.
