---
status: accepted
---

# Attempt recovery once, then reassign with a handoff

Eligible failed work receives one automatic Recovery Attempt under unchanged authority, bindings,
Assignment, and remaining budget. A second failure pauses the Assignment and reassigns it with a
durable Handoff, or creates an Attention Item when safe reassignment is impossible.

## Consequences

Every attempt remains visible and attributable. Recovery cannot refresh budget, widen capability,
change the Proposal, or loop indefinitely. The receiving Agent gets prior context, Artifacts,
verification, and open risks without inheriting false authorship.
