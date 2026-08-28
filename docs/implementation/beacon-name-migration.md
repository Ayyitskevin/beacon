# Beacon name migration

Beacon is the canonical project and local-alpha name. New code and active interfaces use these
identifiers:

- Rust supervisor and crate: `beacond`
- desktop package and crate: `beacon-desktop` / `beacon_desktop`
- Tauri identifier: `dev.beacon.phase0`
- local Alpha CLI store: `.beacon-alpha/beacon.sqlite3`
- preserved Python research package and command: `beacon_legacy` / `beacon-legacy-desk`

The rename preserves two narrow compatibility surfaces. On first desktop launch, a private
same-owner `dev.mosaic.phase0` app-data directory is adopted atomically and its SQLite/WAL family
is renamed. Migration refuses a running legacy supervisor, a symlink or non-private source, and
simultaneous old and new directories. It copies neither credentials nor Workspace records.

The Beacon Legacy Desk accepts its former `MOSAIC_*` endpoint variables after the new
`BEACON_LEGACY_*` variables. These aliases are migration inputs only; diagnostics report the exact
variable selected.

Other remaining Mosaic text is historical: archived design prose, immutable handoff references,
Git history, or captured fixture payloads. It is not an active package, binary, command, namespace,
or product label. The local pre-rename checkpoint is tagged `archive-python-desk-pre-beacon` so the
old implementation can be recovered without retaining two active source trees.
