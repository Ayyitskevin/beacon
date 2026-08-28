# Phase 0 Rust foundation

This checkpoint creates the first new-trunk module beside the preserved Python prototype.

## Interface

`mosaicd::Supervisor::bootstrap` owns four startup behaviors behind one interface:

1. open or create the managed SQLite store;
2. apply the first idempotent schema;
3. create exactly one initial Workspace when the Installation is new;
4. return a serializable startup and diagnostic report.

The desktop, recovery tooling, and tests should call this interface rather than learn SQLite
tables. The implementation currently keeps the store inside `mosaicd`; a store adapter is not
introduced until a second real implementation exists.

## Verified dependency baseline

- Rust 1.97.1 and Cargo 1.97.1 are pinned by `rust-toolchain.toml`.
- `rusqlite` 0.40.2 uses its bundled SQLite feature. Startup uses an immediate transaction so two
  initializers cannot both create the first Workspace. The transaction behavior is documented at
  <https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.transaction_with_behavior>.
- Serde 1.0.229 derives serialization for the typed report. Its derive model is documented at
  <https://serde.rs/derive.html>.
- Serde JSON 1.0.151 encodes the report and persisted event payload.
- The accepted desktop target remains Tauri v2. Its current Linux prerequisites are documented at
  <https://v2.tauri.app/start/prerequisites/>.

This foundation checkpoint originally stopped before installing the Tauri Linux development
packages. They are now present on the development host, and the follow-on signed-out shell is
documented in [phase-0-desktop-shell.md](phase-0-desktop-shell.md). The supervisor remains
independently buildable and does not gain a UI dependency.

The initial schema is plain SQL in `crates/mosaicd/migrations/0001_initial.sql`. Rust includes and
applies that versioned file inside the same immediate transaction that creates the first Workspace.
Workspace names are parameterized, capped at 120 characters, and reject control characters before
the store file is created. The SQL schema independently enforces the length bound.
The schema contains only the Workspace and its typed creation Event; Project persistence waits for
the Phase 1 Project interface. Startup reconciles the exact schema before mutating a non-empty store
and reads every persisted Event back into its closed kind and typed payload before reporting ready.

## Development command

From the repository root:

```sh
cargo run -p mosaicd -- bootstrap
```

The command creates `.mosaic-alpha/mosaic.sqlite3`, creates the `Personal` Workspace only on first
run, and prints the diagnostic event feed as JSON. Repeating it reuses the same Workspace.
