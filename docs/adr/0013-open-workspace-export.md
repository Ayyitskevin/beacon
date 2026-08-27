---
status: accepted
---

# Transfer a Workspace openly and reconnect authority explicitly

Beacon exports a complete Workspace as a versioned, openly documented package that preserves the
records and provenance needed for independent inspection or import. Secret values, live credential
handles, transient locks, and machine-specific authority are excluded. Import restores the safe
record first, then builds a Reconnect Plan for explicit Operator review.

## Consequences

Export is not backup, federation, or a database copy. It does not mutate the source Workspace.
Missing provenance or detected secret material makes export fail closed. Another implementation can
read the format without depending on Beacon internals.

Imported history, Artifacts, Agent identities, and configuration are immediately inspectable, but
Residents, Routes, Workers, Connectors, Plugins, secret references, and external authority remain
inactive. Beacon may detect compatible resources and reuse an existing secret reference the
Operator already approved on the current Installation. It presents those matches in one review and
may activate the accepted set together. It never guesses, copies a secret, or restores authority
without confirmation.
