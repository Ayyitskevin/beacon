---
status: accepted
---

# Default to local inference and grant cloud use per Project

Beacon may discover local model Routes automatically, but they remain ineligible until the Operator
activates them. Activated local Routes are preferred by default. A cloud provider, Route, and
permitted data scope must be enabled explicitly in Project Policy before any cloud Invocation.
This keeps private work local without preventing Projects from choosing frontier cloud capability.

## Consequences

Discovery and installation do not grant eligibility. A Workspace credential does not make its
provider eligible everywhere. Cloud permission is Project-scoped, attributable, and independently
revocable. Missing or failed local capacity is reported honestly and never becomes an implicit
cloud fallback.
