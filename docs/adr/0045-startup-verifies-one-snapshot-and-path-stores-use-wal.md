---
status: proposed
---

# Startup verifies one snapshot and path stores use WAL

Beacon's startup integrity passes enumerate immutable records and then compare the
enumeration with store totals. When those reads use separate autocommit snapshots, an honest
writer can commit between them and make startup falsely report a mismatch. Some startup work is
also necessarily mutating: interrupted dispatch recovery and expired-lease reconciliation may
need to write before the store can be judged.

A long read snapshot must coexist with ordinary writers. Path-backed stores previously opened in
SQLite's rollback-journal mode, which does not provide that concurrency shape. Changing the mode
to WAL is persistent, can create `-wal` and `-shm` sidecars, and changes how an operator must copy
or recover a store. That consequence is part of the decision, not an implementation detail.

## Decision

Beacon runs startup in two ordered transactions. First, one atomic reconciliation transaction
performs interrupted-dispatch recovery and expired-lease reconciliation and commits. Then one
deferred, read-only transaction owns every startup verification enumeration and Totals query, so
all passes judge one SQLite snapshot. Recovery may remain committed when later verification
refuses the store; it may not repair immutable owner records or create authority.

Failure inside the joint reconciliation transaction rolls back dispatch and lease writes
together. Once that transaction commits, a later verification refusal does not roll it back.
Therefore a refused startup may already have changed the store, and operators must treat startup
itself as mutating even when it ultimately refuses to open Ready.

Managed path-backed stores use persistent SQLite WAL on supported same-host local storage.
Non-file in-memory stores are the explicit exception and retain SQLite's `memory` journal mode.
WAL is not a network-filesystem feature, and Beacon does not support placing its managed store on
storage whose locking and shared-memory behavior is not SQLite-compatible local storage. A
refused open may already have established WAL before schema or state verification refuses.

The database main file alone is not a live backup: committed state may still reside in its WAL.
[ADR 0015](0015-encrypted-safety-snapshots.md) continues to own encrypted Safety Snapshot scope,
verification, retention, and key handling, but supplies no WAL copy protocol.
[ADR 0013](0013-open-workspace-export.md) continues to define Workspace export and explicitly
states that export is not backup or a database copy.

This ADR remains **Proposed**. Under R45.6 and R45.7, Kevin's sign-off at merge accepts the
persistent store-format decision and this operating text together; this record does not accept
its own decision.

## Operating consequences

The following is proposed operating text, not evidence that Beacon has executed or passed a
backup drill.

- Identify and quiesce every process and connection that can access the store, including
  read-only connections, or use a SQLite-aware snapshot operation and verify that it completed.
  Readers pin the WAL even when they never write.
- Treat the SQLite backup API as complete only after the backup step reports `SQLITE_DONE` and
  `backup_finish` succeeds. An API call or command exit without that completion state is not a
  completed backup.
- For the offline-copy route, verify checkpoint completion first. A checkpoint can return busy or
  leave work pending; record its returned completion status, not only the command's exit. Keep
  every connection quiescent through checkpoint, connection close, and the complete copy. A
  successful checkpoint does not reserve a later copy window.
- A long-lived reader, including startup's own verification snapshot, can delay checkpoint
  progress. While readers are open, monitor WAL size and filesystem free space.
- Never call a copy of the main database file alone a completed snapshot of a live WAL store.
  Never treat sequential copies of changing files as one atomic snapshot.
- While a store is open, `-wal` holds committed pages that have not been checkpointed and `-shm`
  is SQLite's transient coordination index, not a second copy of those pages. Protect both as
  store data while the store is open; never delete either sidecar manually, including to clear a
  lock.
- After a crash, preserve the database main file and its `-wal` and `-shm` family together. Let
  SQLite recover the family in one private writable staging location before making a new snapshot
  or relying on the recovered store.
- Never open a live or mutable store with `immutable=1`. That flag is not a substitute for
  quiescence, locking, or recovery.
- Restore every backup, crash-recovery family, and read-only archive into one private writable
  staging location first, then verify it there. Beacon does not bootstrap an archive in place as
  a read-only store.
- Verify a restored snapshot before relying on it. Check a known latest immutable record or
  recorded cut in addition to SQLite integrity checks; `integrity_check` alone can report an
  internally consistent but stale main-file copy.
- Include WAL size and filesystem free space in disk-pressure diagnostics. The current runtime
  setting `synchronous=FULL` is measured, not pinned by the product; changing durability requires
  a separate decision.
- Keep managed stores on same-host local storage with SQLite-compatible locking and shared
  memory. Do not use WAL as a network-filesystem interoperability mechanism.

## Consequences

- An honest concurrent write cannot make startup compare an enumeration from one database state
  with Totals from another.
- Reconciliation is durable before verification begins; a later refusal reports invalid owned
  state without undoing legitimate recovery.
- One writer remains SQLite's limit. WAL permits that writer to commit beside the startup read
  snapshot; it does not make writes concurrent with one another.
- Operational copying, crash preservation, and restore verification must include the WAL
  relationship. Existing export and encrypted-snapshot decisions keep their separate scopes.
