# Beacon governance

Beacon is open source and welcomes code, design, documentation, testing, Agent Packs, and other
useful contributions.

## Product Steward

Kevin Lee is Beacon's Product Steward. The Product Steward has final authority over product
direction, core safety boundaries, releases, project governance, and unresolved cross-module
decisions.

## Maintainers

Maintainers own defined modules or project areas. They review contributions, keep tests and
documentation healthy, and make routine decisions inside the accepted blueprint and ADRs.
Maintainer authority does not override the Product Steward or Beacon's product authority model.

## Decision process

Small, reversible changes use normal review. A moderate change is any non-trivial product or
repository change that does not enter a high-stakes category. It requires review by at least one
person or agent who did not author the change before merge. The review binds to the exact commit
and records re-runnable verification, including failures, skips, and warnings.

Changes to public contracts, domain language, security, authority, persistence, compatibility, or
governance are high-stakes, as is any change for which AGENTS.md requires human review. A
high-stakes change requires a written proposal, an updated decision record, one non-author review,
and explicit Product Steward sign-off before merge. An automated gate and an author's own review
cannot substitute for the non-author review or Product Steward sign-off.

Evidence settles technical disagreements when possible. The Product Steward decides when a
material disagreement remains.

### Recorded implementation-only security disposition: A-G1-03

On 2026-09-02 Kevin approved all recommendations in A-G1-03 as staged security work, including the
20–30 hour planning correction and exact-commit non-author review before merge. This is an
implementation-only correction to the private materialization mechanics already decided by
[ADR 0033](docs/adr/0033-install-the-sample-project-as-inert-managed-state.md): the destination,
bundle, persistence, recovery, compatibility, and public-interface decisions remain unchanged, so
ADR 0033's text remains unchanged. Slice 1 binds staging creation, initialization Git processes,
failure cleanup, and publication to held directory descriptors. Later slices still own
descriptor-relative adoption verification, recursive synchronization, and remaining identity-drift
races; accepting this first slice does not dispose of them.

## Commercial boundary

The complete local core remains free under the project license. The Product Steward may offer paid
hosting, managed services, and professional support. Official paid offerings must not make local
authority, auditability, recovery, or the extension system artificially incomplete.

## Community

Contributors are treated as collaborators, not free labor. Decisions and review standards should
be visible. Plugins and forks remain free to explore different directions under the license, while
the eventual cleared project name and official release channel follow this governance. Beacon is a
temporary working title and is not approved for a public release or protected software namespace.
