---
status: accepted
---

# Extend Beacon through permissioned out-of-process plugins

Beacon supports Agent Packs, Connectors, Views, Evaluators, and Worker Capabilities through
versioned manifests. Declarative Agent Packs are the first public extension type because they
exercise identity, instructions, tools, Evaluations, and Policy without adding a new transport.
All Plugin types use the public Command, Query, Event, and Worker lease interfaces. Plugins do
not receive raw database access, load arbitrary native Rust into beacond, replace authority, or
create a second execution path.

## Consequences

Untrusted code runs out of process or in a sandboxed portable runtime. Installation and permission
expansion require operator review, packages are pinned by digest, and plugin activity remains
subject to Policy, Proposals, events, and Receipts. Alpha 1 defines and tests the extension seam but
defers third-party execution.

A public registry requires a publisher signature and adds a separate registry attestation covering
manifest integrity, compatibility, and recorded validation results. The signature proves publisher
and package identity. The attestation proves what the registry checked. Neither proves safety.

The Operator may deliberately sideload a local package after reviewing the same manifest plus a
clear source and risk warning. A sideload has no registry attestation. Sideloaded Plugins receive no
weaker sandbox, broader interface, or automatic update path.
