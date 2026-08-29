---
status: proposed
---

# Preserve Team formation separately from current eligibility and Mission leadership

A Team is a durable, named, Project-scoped group of one or more distinct Resident identities. Its
UUID is identity; its name is a label and need not be unique. The first product path is an explicit
authenticated Operator Command because Beacon does not yet have a Resident command subject,
Policy adjudication, or an Agent runtime. The audit record therefore says `operator`; Beacon does
not invent Resident attribution for an action the Operator performed.

Formation names each selected Resident and the exact current Resident Configuration used to admit
that member. Every selected configuration must still be current and eligible inside the same
immediate transaction. Membership combines neither capabilities nor authority, and the Team has
no permanent lead. Team Lead selection remains a separate, Mission-specific Policy decision.

## Consequences

The Team and its ordered formation roster are immutable. Later lifecycle, model-activation, or
configuration decisions do not rewrite formation proof. Team queries derive each member's current
configuration and eligibility, allowing the interface to distinguish historical membership from
present readiness. A later Mission must bind the exact then-current configuration rather than
reusing formation evidence as authority.

Each accepted formation command creates a new Team identity, even when another Team has the same
name and roster. This is explicit command semantics, not name-based idempotency. Concurrent
accepted commands therefore produce distinct, fully formed Teams and separate Events; neither may
partially overwrite the other.

Formation creates no Mission, Assignment, Route, Invocation, Run, credential, budget, network
scope, external effect, or execution authority. Autonomous Resident formation remains unavailable
until a real authenticated Agent command path and Policy module can attribute and adjudicate it.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.
