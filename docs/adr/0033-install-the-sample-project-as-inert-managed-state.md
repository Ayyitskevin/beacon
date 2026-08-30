---
status: accepted
---

# Install the Sample Project as inert, Beacon-managed state

Product Steward ruling, 2026-08-30: Kevin accepted the opt-in Sample Project
implementation direction in the Herdr session and authorized this scoped slice to
proceed. This record makes the persistence, compatibility, and failure semantics
of that ruling explicit. The exact implementation still requires the non-author
review and verification required by `GOVERNANCE.md`.

ADR 0028 selected a bundled, disposable Rust task tracker as Alpha 1's proving
target. It did not decide how a fresh Beacon installation obtains that Project or
how the resulting local state is proved. Treating installation as a Run would be
false: setup does not execute an agent task, consume model capacity, or exercise a
Repository's Declared Checks. Treating the bundle as an ordinary Operator path
would also be false: no Operator repository exists on the fresh-install path.

## Decision

The Sample Project is installed by an authenticated, explicit setup Command. A
separate authenticated Query reports whether the current Workspace can install it
or already has an installation. Neither interface accepts a path, repository
locator, Project identity, authority profile, or executable input. The destination
is derived beneath the supervisor store's private parent:

```text
sample-projects/<workspace UUID>/task-tracker-v1
```

Installation creates an ordinary Project, Charter revision, and local Repository
binding. Its Charter is `supervised`, `no_spend`, and `external_effects =
prohibited`. Its Repository declares real format and test checks, but installation
does not run them. It creates no Run, Receipt, Proposal, Ruling, model request,
network request, or external effect.

Schema version 17 adds one immutable `sample_project_installations` row per
Workspace and admits one reconciled `sample_project.installed` Event. The row
binds the exact Project, Charter revision, Repository revision, managed path,
initial Git revision, bundle version, content digest, provenance source, and
installation time. Project and Repository creation keep their existing Events;
the installation Event records the setup-specific aggregate. Startup refuses
missing, duplicate, unknown, or mismatched installation evidence.

Bundle v1 is a permanent descriptor, not a moving `latest` alias. Its ordered
paths and bytes, SHA-256 canonicalizer/digest, managed directory name, and initial
Git revision never change. Tests pin the numeric version, digest, and revision to
literals. A future bundle is additive: it receives a new versioned descriptor and
directory while v1 validation remains available for existing stores. Replacing v1
constants or bytes in place is a compatibility violation.

Git initialization runs with an empty template and a closed environment containing
only the narrow locale, path, configuration, lock, and prompt settings Beacon
supplies. Ambient Git directory, worktree, index, object, configuration, hook, and
identity variables are not inherited. The initial commit has fixed content,
identity, time, message, branch, and revision. An existing destination is adopted
only when its private directory, exact bundled bytes, tracked paths, clean index
and worktree, and frozen revision all match; equal files under a different commit
are refused.

## Publication and recovery

Filesystem and SQLite publication cannot be one transaction. Beacon therefore
uses a narrow recoverable sequence:

1. authenticate and verify the compiled bundle identity;
2. write into a new owner-only staging directory;
3. initialize and verify the exact Git repository;
4. fsync every file and directory in that tree from leaves to root;
5. publish with Linux `renameat2(RENAME_NOREPLACE)` and fsync the containing
   directory;
6. publish all domain rows and Events in one immediate SQLite transaction.

A failure before rename removes only the random staging directory and publishes no
database state. A database failure after rename can leave an exact orphan at the
derived private path. A later authenticated install verifies that complete frozen
identity and adopts it; it never overwrites or repairs a differing destination.
This is the compensating action for the two-system boundary. Repeat and concurrent
requests converge on the one immutable installation or fail closed; neither may
create a second Project.

Migration 0017 rebuilds the closed Event-kind table while preserving prior
sequences, payload bytes, and timestamps. It is transactional: rollback restores
the exact v16 schema and evidence, and forward startup verifies the v17 schema and
foreign keys. Once v17 state has committed, an older binary is not a supported
reader. Removing the feature must preserve or explicitly migrate its immutable
evidence; dropping it silently is not a reversal.

## Public boundary and consequences

Rust owns the wire types. Tagged enum fields are camelCase, and the desktop adapter
must validate exact keys and values before rendering them. Invalid or widened
payloads fail at the adapter rather than becoming trusted UI state. Transport
errors expose stable refusal codes and do not leak local paths or Git output.

This decision adds durable setup state and a small amount of installation-specific
orchestration, but no execution authority. The managed repository is private base
material for later disposable worktrees. Any future action that runs its Declared
Checks remains governed by the existing Mission, preflight, sandbox, Run, Receipt,
and Ruling decisions; this ADR does not bypass or weaken them.
