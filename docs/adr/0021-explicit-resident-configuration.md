---
status: accepted
---

# Make Resident Configuration explicit without granting execution authority

A Resident Configuration is a separate, append-only Operator decision. It binds one Resident to
one exact current local-model Resource Activation, an explicit set from Beacon's closed Capability
catalog, and the Alpha `no_spend` boundary. It may be created for any non-retired Resident, but a
Resident becomes eligible for new Team formation only while its lifecycle is active and that exact
model activation remains current and active.

Configuration is neither lifecycle nor execution authority. It creates no Route, Team, Mission,
Assignment, Invocation, Run, credential, data scope, network permission, external effect, or
approval. Role-based Capability suggestions exist only in the desktop editor and require explicit
Operator confirmation. Team leadership remains Mission-specific policy selection rather than a
permanent property of the Lead role.

## Consequences

Each set, clear, or reconfiguration appends an Operator-attributed decision and exactly matching
typed Event in one immediate transaction. The command names the exact preceding configuration
decision, so concurrent or stale screens cannot overwrite newer intent. The database preserves one
linear decision chain per Resident and rejects updates or deletes from decisions, bindings, and
Capability rows.

Deactivating a model never rewrites Resident Configuration. A later activation for the same model
has a new activation ID and cannot silently resurrect the old binding; the Operator must append a
new configuration decision. Suspending or retiring a Resident likewise changes derived Team
eligibility without rewriting its configuration history. Retirement refuses all later
configuration decisions.

Startup fails closed unless every configuration chain, exact binding, Capability set, and typed
Event reconcile. The desktop and authenticated transport expose eligibility as derived status and
keep the fixed no-spend/no-execution boundary visible.

Accepted 2026-08-29 by the Product Steward. The reviewed branch merged to `main` as `3513ec3` on 2026-08-28; the Steward ruled on 2026-08-29 that every ADR merged under this convention is accepted.
