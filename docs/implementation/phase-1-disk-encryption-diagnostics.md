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
block-stack walk at most 8 deep and 64 nodes with a visited set. The `slaves/` iterator is bounded
**before it is consumed**, not while: a pre-child budget check is only as good as the loop honouring
it, and collecting every entry first and checking afterwards still walks the whole directory.
Enumerating costs no open, so no fixture can observe that difference from outside — bounding the
iterator makes the property structural instead of asserted. Recorded observations are cut at 200 characters
with the `truncated` flag set, stripped of control characters, and capped at 32 entries.

## The shape

The report carries a **conclusion**, not a sentence: a closed `state`, a closed `ReasonCode`, a
bounded list of `Subject`s (each with a role, a value capped at 24 characters, and a `shortened`
flag), and a count of the subjects that did not fit. There is no free text on the wire. The human
sentence is rendered from those fields by a total function — once in Rust for logs, once in
TypeScript for the panel — and it cannot overflow because its inputs cannot.

That is a change of approach, not another patch. Five review rounds found the same class in a new
place each time: a conclusion or a failure message composed from unbounded parts and then cut. Every
previous fix bounded the strings that had overflowed. This one bounds the type that admitted them,
so there is no longer anywhere for an unbounded value to enter a conclusion.

Evidence keeps its two text fields and gains `truncated`. The panel prints "…(shortened)" from that
flag rather than looking for an ellipsis — a redaction marker in one field previously made a silent
truncation in another look guarded.

What a reader can expect: the state and the reason are drawn from fixed sets and can be matched on;
the sentence is a rendering of them and may be reworded; a subject that was cut says so; a subject
list that was capped carries the number it dropped. What a reader cannot expect: that a subject is
the complete value, or that the sentence is stable text to parse.

## The rule

A **physical leaf** is a node with no `dm/` directory and no parents: a real disk or partition. A
device-mapper node with no parents is *not* a leaf — it is a mapping whose backing device Beacon
never saw, which is an unread stack rather than an encrypted one, and real LUKS never looks like
that.

`enabled` means every branch terminated at a physical leaf, every node was readable, and **every**
one of those paths passed through a `CRYPT-` layer. `disabled` means every branch terminated at a
physical leaf, every node was readable, and at least one of those paths had no crypt layer; the
report names those devices. Any read, metadata, or budget failure anywhere is `unknown`, with the
path in the reason. No read failure can produce `enabled` or `disabled`.

A crypt layer is a fact about one path, never a verdict about the volume. A crypt layer **above** a
plain leaf covers that leaf; one on a **sibling** branch does not. A logical volume over one LUKS
member and one plain member has half its extents in the clear and reports `disabled`.

Termination is proved **per branch**, not counted globally. A branch that ends in a cycle, a budget
stop, an unreadable node, or a mapping with no backing device makes the whole answer `unknown` — a
sibling that finished, encrypted, proves nothing about the branch that did not. If nothing
terminates the stack, "every leaf is covered" is true of no leaves, and that vacuous truth is
`unknown` too.

**Broken is not absent, anywhere.** `NotFound` is believed only when it is corroborated: a store
whose `canonicalize` says `NotFound` is absent only if `symlink_metadata` agrees, and a `slaves`
directory is absent only if `symlink_metadata` never saw the link. A link that resolves to nothing
is an error at both.

The `disabled` reason says what was found rather than denying it: "at least one path to disk has no
crypt layer: 259:2", or "pv0 is encrypted; 8:17 is not" when a crypt layer was observed on another
branch. An operator who can see `CRYPT-LUKS2` in the same report's evidence and reads "found no
encryption" does not conclude the report is subtle.

`disabled` ships with a `notVerified` list naming what the reading could not see: drive-firmware
(OPAL/SED) encryption, filesystem-level encryption such as fscrypt or ext4 encryption, and
encryption applied beneath this host.

## What is read, in order

1. `canonicalize` the store path, falling back to its parent directory **only** when that fails
   with `NotFound` **and** `symlink_metadata` agrees the entry is not there — the store does not
   exist before first open. Any other error, and a link whose target is gone, describes a store that
   is there and could not be resolved; substituting its parent would answer about a different
   subject, so it is reported instead.
2. `stat` it for `st_dev`, decoded with `libc::major`/`libc::minor` rather than an open-coded shift.
3. `/proc/self/mountinfo`, parsed per `proc_pid_mountinfo(5)`: six fixed fields, a variable run of
   optional fields, the `-` separator, then filesystem type, mount source, and super options. Mount
   points are unescaped from their three-digit octal form. Selection is the longest
   path-**component** prefix of the target; on a tie the last entry wins, because a later mount
   shadows an earlier one at the same point.
4. Corroboration: when `st_dev` names a real block device, it must equal the selected entry's
   device. A disagreement is `unknown`, not a confident answer about the wrong volume.
5. `/sys/dev/block/<major>:<minor>/`, walking `slaves/` depth-first to every physical leaf, reading
   `dm/uuid`, `dm/name`, and `loop/backing_file`. Each branch carries whether a crypt layer has
   been seen on the path to it. The memo of settled branches is keyed on the device **and** that
   flag — the same disk can be reached by a covered and an uncovered path, and pruning the second
   visit is how a plain branch would disappear — and it holds only branches already *proved
   terminated*, so a diamond is reused while a device found on the current path is a cycle and
   terminates nothing.

Presence is a three-way answer everywhere, never a boolean: corroborated `NotFound` is absence, any other
metadata error is a reason. Directory checks follow symlinks deliberately — every entry under
`/sys/dev/block` is a symlink to the real device directory — so a link that resolves to nothing is
an error, not an absence. `slaves` uses `read_dir` itself as the presence check. Both budgeted reads
take one byte past the budget and refuse anything larger, because `take(budget)` alone returns the
first `budget` bytes of a larger file and looks exactly like a whole small one.

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
count, and that only `enabled` names a mechanism. Both rendered fields — `mountPoint` and
`mechanism` — are bounded at 200 characters on both sides, so a reason carrying a long sysfs path
cannot make a whole valid report unreadable at the boundary. The mount point is bounded but not
redacted: it is how the operator recognises the volume, and it is a directory they chose rather
than an identifier read out of the kernel. No new `LocalErrorCode` and no new
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
