# Disk encryption — ship the explicit unknown-only v1

## What you are accepting

Merging this accepts option **(b)** from the 2026-09-02 ruling: the v1 Query reports only
`unknown`. It records bounded host observations and preserves a specific failure cause when a read
fails, but it does not promote the guarded detector candidate's `enabled` or `disabled` inference
into a production claim. A candidate confident state becomes `detection_not_evaluated`; it is not
absence, failure, or a guessed negative.

This also accepts three consequences of that decision.

1. **The report remains bounded by construction.** The closed Rust report, checked deserialization,
   strict TypeScript adapter, total renderers, canonical device numbers, truthful omission counts,
   and iterator budget all remain. Option (b) narrows the production result; it does not weaken the
   interface or discard the seven review rounds that made its limits executable.

2. **The detector candidate remains an executable specification, not production evidence.** Its
   fixture suite still distinguishes a missing path from a broken one, requires every stack branch
   to terminate, rejects unreadable or parentless mappings, preserves mixed covered/uncovered
   conclusions, and stops at structural read budgets. Candidate conclusions do not enter the v1
   report's evidence.

3. **Criterion 38 remains partial.** The authenticated Query, warning panel, bounded observations,
   operating-system guide, and recheck action ship with an honest unknown answer. This merge does
   not claim that Beacon can yet establish whether the live-data volume is encrypted.

## What is in the branch

The branch starts from PR #35's actual head `a4392f0`, rebased onto `b8cd5fb`. It carries the daemon
reader and closed report over the authenticated local transport into the desktop warning panel,
plus the fixes and executable guards accumulated through seven review rounds.

The final option-(b) commit adds the production `unknown_only_v1` boundary and the closed
`detection_not_evaluated` reason on both sides of the wire. The real-transport test asserts that no
confident candidate state crosses the socket. Two confident candidate fixtures independently
assert the same boundary while retaining non-empty observations.

The commit also closes the remaining review gaps: canonical `major:minor` device spellings and
truthful omission counts agree in Rust and TypeScript; every reason must admit a maximum-width
checked conclusion; a production `BoundedChildren` type makes the fan-out cap structural; and the
host-dependent fixture selects a fixture-owned non-anonymous device when `TMPDIR` lives on an
anonymous filesystem.

## Verification

Every mutation from the seven review rounds was replayed against the named guard. All were observed
red, including first-encrypted-sibling masking, dangling-store substitution, swallowed read errors,
overlay confidence, noncanonical device aliases, dishonest omission counts, bypassed v1
normalisation, unbounded directory fan-out, and the `/dev/shm` host-device dependency.

`scripts/gate all` is the merge gate. Focused Rust candidate, real-transport, `/dev/shm`, desktop
adapter, panel, and TypeScript-build checks are recorded in `C19-gpt1-build-report.md`.

## What option (a) would add

Option **(a)** is a later type-first detector rewrite. It must define the accepted Linux volume
model before parsing sysfs, make unreadable and unrepresentable states impossible to confuse with a
physical leaf, prove that every reachable path is covered, and validate the result against a real
encrypted volume rather than fixtures alone. Only after that evidence exists may `enabled` or
`disabled` cross the production Query; macOS and Windows detection remain separate work.

## What this closes

This closes the ruled v1 row without converting uncertainty into reassurance. Operators can see
that Beacon checked, what bounded observations it made, why a read could not answer when one failed,
and how to recheck or open the platform guide. The interface says exactly what the accepted
implementation establishes: disk-encryption detection was not evaluated into a confident claim in
this version.
