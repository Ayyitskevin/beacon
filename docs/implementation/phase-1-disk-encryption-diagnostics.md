# Phase 1 live-data disk-encryption diagnostics

This checkpoint implements ADR 0017 and the Alpha criterion at `docs/BLUEPRINT.md:914-917`: Beacon
reports the operating system's disk-encryption status for the volume holding live data as enabled,
disabled, or unknown, shows the evidence it read, and never describes the open live store as
Beacon-encrypted.

## Boundary

`DiskEncryptionService::status()` is an ephemeral read. It spawns no process, opens no socket,
writes nothing, changes no operating-system setting, persists nothing, and emits no event or
Receipt. Every call re-reads live state, so "recheck" is the same Query rather than a second
command. The account session is checked before and after the reads, as local-model discovery does;
a result gathered under a session that has since closed is withheld.

Budgets: `/proc/self/mountinfo` at most 4 MiB and 16,384 entries, each sysfs file at most 4 KiB, the
block-stack walk at most 8 deep and 64 nodes with a visited set. Recorded observations are truncated
to 200 characters, stripped of control characters, and capped at 32 entries.

## The asymmetry

Nothing reports `enabled` except a device-mapper `CRYPT-` UUID this module read itself. Every
failure to read reports `unknown` with its reason. No read failure can produce `enabled` or
`disabled`. `disabled` is a proven negative — a block stack read end to end with no crypt layer —
and it ships with a `notVerified` list naming what the reading could not see: drive-firmware
(OPAL/SED) encryption, filesystem-level encryption such as fscrypt or ext4 encryption, and
encryption applied beneath this host.

## What is read, in order

1. `canonicalize` the store path, falling back to its parent directory when the store file does not
   exist yet.
2. `stat` it for `st_dev`, decoded with `libc::major`/`libc::minor` rather than an open-coded shift.
3. `/proc/self/mountinfo`, parsed per `proc_pid_mountinfo(5)`: six fixed fields, a variable run of
   optional fields, the `-` separator, then filesystem type, mount source, and super options. Mount
   points are unescaped from their three-digit octal form. Selection is the longest
   path-**component** prefix of the target; on a tie the last entry wins, because a later mount
   shadows an earlier one at the same point.
4. Corroboration: when `st_dev` names a real block device, it must equal the selected entry's
   device. A disagreement is `unknown`, not a confident answer about the wrong volume.
5. `/sys/dev/block/<major>:<minor>/`, walking `slaves/` breadth-first, reading `dm/uuid`,
   `dm/name`, and `loop/backing_file`.

Filesystems whose encryption is not decided by a block device under this host — tmpfs, ramfs,
overlay, FUSE, and network filesystems — are `unknown`, each with its own reason. So is a loopback
image: the reason records the backing file so a later slice can follow it. btrfs and ZFS report an
anonymous device number, so the block device is taken from the mount source instead.

Non-Linux platforms report `unknown` with the reason "not detectable on this platform". FileVault
and BitLocker need process spawning or platform APIs this slice excludes; `BLUEPRINT.md:914` scopes
the criterion to "when detectable".

## Sources, and one that is not local

`proc_pid_mountinfo(5)` documents the mount-table format. The device-mapper UUID convention
(`CRYPT-<TYPE>-<uuid>-<name>` for crypt targets) is written by cryptsetup. `dmsetup(8)` on the
measurement host documents that device-mapper devices carry a UUID and that UUIDs are read as
subsystem-prefixed constituents, but it does not spell out the crypt prefix, and cryptsetup is not
installed there. That prefix is therefore recorded in the module as observed convention rather than
a locally citable format — which is why the match is on the generic `CRYPT-` prefix with the type
token derived from the UUID, not on an allowlist. An unfamiliar crypt type still reports `enabled`
with an honest label instead of being silently missed.

## What never reaches the record

Evidence carries the crypt UUID's type prefix and nothing else. The identifier reaches the record by
more doors than the obvious one — many distributions name the mapping `luks-<uuid>`, so `dm/name`
and the mount source carry it too — so any hexadecimal run of twelve characters or more is redacted
on the way in, closing every door at once rather than one per source.

## Interfaces and presentation

One closed report crosses the Rust service, the authenticated Unix socket, the Tauri command, and
the strict TypeScript adapter, which re-checks the three states, the bounded text, the evidence
count, and that only `enabled` names a mechanism. No new `LocalErrorCode` and no new
`LocalTransportError` variant were needed: detection failures are a truthful `unknown` result, not
an error, and the only way the Query fails is a closed session.

The Installation inspector hosts the status line. This is a **Diagnostic of the Installation**, not
an **Attention Item**: `CONTEXT.md` reserves Attention Items for conditions where the Operator or a
named Agent can take a concrete next action and gives only that object the right to request an
operating-system notification. The actor here is the operating system, and a notification right
cannot also be the unobtrusive warning ADR 0017 requires. The warning has no dismiss control, so the
condition ends it rather than the operator.

The setup guide is bundled text rather than a link: this inspector reports that external network
activity is off, and a warning that opened a browser would contradict it. It adds no Tauri plugin,
no capability beyond the Query's own, and no change to the content-security policy. It names the
destructive Linux path — converting a running installation in place — states that this can destroy
all data on the volume, and points at the distribution's own documentation instead of supplying a
command.

## Verification

From the repository root:

```sh
cargo test -p beacond --test disk_encryption
cargo test -p beacond --lib disk_encryption
cargo test -p beacond --test local_transport authenticated_client_reads_live_data_encryption_over_both_real_transports -- --exact
cargo test --workspace --all-targets
scripts/gate all
```

From `apps/desktop`:

```sh
npx vitest run src/App.test.tsx -t "warns persistently"
npx vitest run src/App.test.tsx -t "never describes the open live store"
npx vitest run src/session.test.ts -t "refuses a live-data encryption report"
```

Every test in this slice was first run against the wrong answer it exists to kill, and each was
observed failing before the reader was written to satisfy it. The mutants and their observed
assertions are recorded in the build report for this slice.

## Evidence limits

The reading is proved against fixtures, not against a live encrypted volume: the measurement host
has no device-mapper crypt device, so `enabled` has never been observed against real sysfs. The
`CRYPT-` prefix set is observed convention, not a local citation. macOS and Windows detection does
not exist. The desktop assertions run under jsdom, which lays out nothing, so rendered contrast and
visible focus for the new status line remain unproven, exactly as
`docs/evals/2026-08-30-desktop-accessibility-baseline.md` records for the rest of the interface.
