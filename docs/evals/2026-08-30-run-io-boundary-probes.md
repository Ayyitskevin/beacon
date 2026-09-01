# Run-I/O boundary: static evidence and gated hostile probes

Status: **static evidence recorded; P0-P3 not executed**.

Recorded 2026-08-31; the filename is retained from the approved BCN-41 artifact
fence.

This record is pinned to Beacon
`0a30798a58349e1fc338ababe0b107af97eb567f`. It separates facts readable from
repository bytes from behavior that only a runtime probe can establish. Nothing
in this record is a probe result, an accepted ADR, or execution authority.

## Marking convention

- **[S]** is static evidence derived from the exact Git objects below.
- **[UNVERIFIED]** is runtime behavior not observed for this record.
- **[PROPOSED]** is a requirement in ADR 0035, not current behavior.

No probe, test, build, gate, sandbox child, listener, fork, restriction, UDP
send, or production command was run to produce this record.

## Source pins

| Blob | Path at `0a30798a58349e1fc338ababe0b107af97eb567f` |
|---|---|
| `f0e9dce27a995001daaa7f4fc034df106aeaba5a` | `crates/beacond/src/sandbox/landlock.rs` |
| `0a4cfabfca7fda94af909990cc4af141e4f4b34d` | `crates/beacond/src/sandbox/probe.rs` |
| `ada36651da6c88c72c09517917c62e8f7d5df73b` | `crates/beacond/src/sandbox/mod.rs` |
| `09e2d86687b1a5e36d091cab961e085014fb0184` | `crates/beacond/src/sandbox/worktree_launcher.rs` |
| `5842d7fbd3fff6980ef0c60a28e66008ce51355a` | `crates/beacond/src/sandbox/bounded.rs` |
| `41264cce1b4729e3db114197757da65457b1fc90` | `crates/beacond/src/declared_checks.rs` |
| `4e97e27f867a06737b1e0f6c74d731c0b8ff57db` | `crates/beacond/src/check_preflight.rs` |
| `8015548b62953b54c0b1276aad1c194d5e02ed08` | `crates/beacond/tests/declared_checks.rs` |
| `c63f9d180f242879b078f6fe46a40b66d4f7cfb9` | `crates/beacond/tests/sandbox.rs` |
| `c5946507a52f9851ac152b7c95e49b8713f10c34` | `crates/beacond/tests/check_preflight.rs` |
| `55c8f0173ec3174f53af7052f5419a6c2b040b20` | `crates/beacond/src/local_transport.rs` |
| `e1eef3e818754dd9869f2d3d0685cc873999f6ca` | `apps/desktop/src-tauri/src/lib.rs` |
| `d555d672596bcf899b052667aee3dc25e9281fd7` | `apps/desktop/src/App.tsx` |
| `4628760833cdafbb85c17c8cb7455b3a2d7067fa` | `SECURITY.md` |
| `ec5c029973eb22bbcadd9825c5f70f892c626aa4` | `GOVERNANCE.md` |
| `745e1007a5344608dbb9f3661a232a98b6dfa3a1` | `docs/BLUEPRINT.md` |
| `e819283ca3d0ea5cb51f40b1dd69551e55d76912` | `docs/adr/0029-landlock-sandbox-proven-before-trusted.md` |
| `08074c3de72ef978d15042b12a8f8f9f1283fa6e` | `docs/adr/0031-execution-receipt-v2.md` |
| `b94836d09f2ac78a231312fcab7a15255d29a557` | `docs/adr/0032-descriptor-bound-working-directory-containment.md` |
| `04bb8096785327ee49e001a9d0bbf0b64cbbe50d` | `docs/adr/0034-immutable-proposal-v1-authority-bytes.md` |
| `048ad5a970e9b1c1d51b99c77921bde6e6930718` | `scripts/gate` |

External primary source checked on 2026-08-31: the Linux kernel's
[Landlock userspace API documentation][landlock]. It defines TCP network rights
from ABI 4, UDP bind/connect-send rights from ABI 10, filesystem device-ioctl
rights from ABI 5, and pathname UNIX-socket resolution rights from ABI 9. It
also states that a restricted thread's future children inherit its Landlock
domain. Those interface definitions are static reference evidence; they do not
establish which ABI a target host is running.

## S1 — readable authority is host-wide

- **[S]** `landlock.rs:55-58` defines `READ_RIGHTS` as execute, read-file,
  and read-directory.
- **[S]** Both constructors grant those rights at `/`: the path builder at
  `landlock.rs:128-138`, especially `:134`, and the descriptor-based worktree
  builder at `landlock.rs:146-154`, especially `:152`.
- **[S]** Only write authority is narrowed beneath selected targets
  (`landlock.rs:60-70`, `:135-136`, and `:153`).
- **[S]** Capability detection depends on retaining a system read:
  `probe.rs:90-93` selects `/proc/self/cmdline`,
  `mod.rs:151-154` includes `read_system_path` in `filesystem_held()`, and
  `tests/sandbox.rs:30-34` asserts it.
- **[S]** `HANDLED_FS` is assembled once at `landlock.rs:72-74`; rights outside
  that mask are explicitly ungoverned.
- **[S]** Current upstream Landlock defines filesystem rights beyond bit 14:
  device `ioctl` from ABI 5 and pathname UNIX-socket resolution from ABI 9.
  Beacon's `HANDLED_FS = 0x7fff` does not name them. Whether the target host
  exposes the corresponding ABI remains unverified.
- **[UNVERIFIED U1/U2]** The Landlock ABI and therefore the applicable
  filesystem-right set on the target AppImage host class have not been measured
  here. P0 settles the host half; the upstream ABI table supplies the mapping.
- **[UNVERIFIED U4]** The minimum runtime/toolchain read set that still permits
  representative execution is unknown and is not part of P0-P3.

Narrowing the `/` grant without changing the probe contract would either break
execution or leave detection asserting a broader property than the ruleset.
That pair must be designed and verified together.

## S2 — the governed network mask is TCP-only

- **[S]** `landlock.rs:51-53` defines only TCP bind and connect rights.
- **[S]** `landlock.rs:157-166` places only those two bits in
  `handled_access_net`; below ABI 4 it uses zero.
- **[S]** `landlock.rs:27-29` names ABI 4 as the TCP floor, and
  `:124-127` plus `:226-229` require callers to inspect whether restriction
  took effect rather than assume it.
- **[S]** Production requests that mask in `worktree_launcher.rs:69`; detection
  requests it in `probe.rs:57`.
- **[S]** ADR 0029 lines 19-24 and 58-64 plus `SECURITY.md:49-52` state the
  ruleset-scoped limit: current Beacon code requests TCP rights only and no
  named-destination enforcement exists.
- **[S]** The target is broader: `BLUEPRINT.md:781-786` requires network denied
  by default and granted only to named destinations.
- **[S]** Upstream Landlock defines UDP bind and connect-send rights from ABI
  10, but Beacon neither includes those bits nor adds UDP port rules. Landlock
  port rules alone also do not express a hostname or IP allowlist.
- **[UNVERIFIED U3]** Whether a sandboxed child can emit UDP under a
  production-shaped ruleset is not established here. P1 settles it.

## S3 — the UDP Receipt gap is conditional

- **[S]** `declared_checks.rs:367-370` defines the execution-v2 UDP gap.
- **[S]** `declared_checks.rs:490-501` adds it only when at least one record is
  not `NotStarted`.
- **[S]** `tests/declared_checks.rs:420-427` asserts the positive case, while
  `:744-755` asserts that all-`NotStarted` execution has no UDP gap.
- **[S]** Verified preflight adds the same gap unconditionally at
  `check_preflight.rs:617-623`; `tests/check_preflight.rs:307-319` asserts it.
- **[S]** ADR 0031 lines 64-93 records the conditional execution-v2 rule.

Changing this string, conditionality, or nested meaning would alter the frozen
Receipt-v2 contract. ADR 0035 changes no Receipt version and authorizes no such
change.

## S4 — capability is typed and fail-closed

- **[S]** `mod.rs:32-57` defines closed `Enforcing`, `FilesystemOnly`, and
  `Unavailable` variants with `deny_unknown_fields`.
- **[S]** `mod.rs:59-82` preserves distinct unavailable reasons, including
  `ProbeRefused` and `ProbeInconclusive`; `tests/sandbox.rs:68-91` proves they
  are not interchangeable.
- **[S]** Only `Enforcing` admits execution (`mod.rs:84-90`), and
  `worktree_launcher.rs:48-57` refuses every other variant.
- **[S]** `mod.rs:92-119` records a non-empty Receipt capability value and
  distinguishes an observed refusal from no observation. The contract is
  asserted at `tests/sandbox.rs:123-153`.
- **[S]** `mod.rs:136-173` defines the four observations and runs detection per
  call instead of caching a machine claim.

Any later network state must be a new reviewed typed shape. It cannot widen the
meaning of `Enforcing` or compress availability into a boolean.

## S5 — production and probe path resolution differ

- **[S]** Production opens the worktree root with `O_NOFOLLOW`
  (`worktree_launcher.rs:17-22`) and resolves working directories with
  descriptor-bound `openat2`, `RESOLVE_BENEATH`, and
  `RESOLVE_NO_MAGICLINKS` (`:24-35` and ADR 0032 lines 97-123).
- **[S]** Production builds the write grant from that retained descriptor
  (`worktree_launcher.rs:48-84`).
- **[S]** Generic `Ruleset::grant()` instead opens its path with
  `O_PATH | O_CLOEXEC`, without `O_NOFOLLOW` (`landlock.rs:190-201`).
- **[UNVERIFIED U5]** Whether a swapped symlink is followed by that probe-style
  open, while the production-shaped `openat2` path refuses it, has not been
  measured here. P2 settles the flag-level asymmetry; it does not by itself
  prove Beacon's complete launcher path.

## S6 — socket failure can be scored as enforcement

- **[S]** The child calls `socket()` at `probe.rs:255`. Any negative result sets
  `BIT_REFUSED_TCP` at `:256-260`, without reaching `connect()` at `:266-273`.
- **[S]** `interpret()` maps a complete filesystem observation plus that bit to
  `Enforcing` at `probe.rs:177-206` when the ruleset reports TCP restriction.
- **[S]** Elsewhere the module uses `ProbeInconclusive` for setup failures,
  including a parent listener failure (`probe.rs:67-74`), and ADR 0029 lines
  38-41 states that no answer is not a negative answer.
- **[UNVERIFIED U6]** It is unknown whether a real `socket()` failure can make
  `Enforcing` reachable, and under which errors. P3 settles this. Until then it
  is a candidate defect, not a confirmed defect.

## S7 — descendant and alternate-launcher closure is unproved

- **[S]** The upstream Landlock API states that a restricted thread's future
  children inherit its domain.
- **[PROPOSED]** ADR 0035 requires one Run-scoped child domain inherited by
  every descendant, with no alternate launcher, inherited environment, or
  working-directory path around it.
- **[UNVERIFIED U7]** Static API inheritance does not prove that every future
  Beacon execution path creates the domain in the correct child or that no
  alternate launcher bypasses it. That production call-graph property requires
  a separately approved G2 implementation and proof plan; P0-P3 cannot settle
  it.

## Reachability boundary

- **[S]** `DeclaredCheckService::execute` is defined at
  `declared_checks.rs:166-224`; no production transport or desktop command
  calls it.
- **[S]** Operator-reachable preflight follows
  `App.tsx:1085-1089` -> Tauri `lib.rs:870-877`, `:1295-1300`, and `:1633-1637`
  -> local transport `local_transport.rs:1022-1026` and `:1419-1424`
  -> `check_preflight.rs:271-305` -> `:596-599` -> `sandbox::detect()`.
- **[S]** Detection binds `127.0.0.1:0` at `probe.rs:67`, forks at `:108`, and
  applies the ruleset in the child at `:228`.

Preflight runs no Declared Check, but an Operator can reach the detector child
and ephemeral loopback listener. Describing the current product as having no
Operator-reachable process or listener would be false.

## U1-U7 — unverified register

| ID | Unverified question | Settled by |
|---|---|---|
| U1 | What Landlock ABI does every exact host in the Product Steward-defined target matrix expose? | P0 once per approved matrix row; one run answers one host only |
| U2 | For each measured ABI, which filesystem rights fall outside Beacon's current handled mask? | P0 host result plus the matching upstream ABI table; a stale local header is inconclusive |
| U3 | Does a sandboxed child emit UDP under the production-shaped ruleset? | P1 |
| U4 | What minimum immutable runtime/toolchain read set still permits representative execution? | A separate, later G1 packet |
| U5 | Does probe-style path opening follow a swapped symlink while production-shaped `openat2` refuses it? | P2 |
| U6 | Can `socket()` failure produce `Enforcing`, and under which errors? | P3 |
| U7 | Does every production execution path arm one Run-scoped domain in the correct child and close alternate-launcher, environment, and working-directory bypasses? | A later G2 implementation and proof plan after ADR acceptance |

## G1 planning record — P0 source complete; all execution approval pending

No command in this section may run without Kevin's exact approval. Approval of
the ADR draft, ordinary documentation work, or “continue” is not G1 approval.
P0's complete measurement source and command template appear below, but this is
not an execution-ready approval request: a later request must bind the exact
target host and instantiate every setup placeholder. P0 may then be approved
separately because it creates no ruleset, child, or listener. P1-P3 are
behavioral design specifications only: this record contains neither their source
bytes nor source digests. They may not be approved or executed from this record.
A later request must inline their complete sources or bind immutable source files
by digest, obtain non-author review of those exact bytes, and ask Kevin for
approval bound to that exact commit and digest set.

### Preconditions

1. Create one dedicated clone at exact base
   `0a30798a58349e1fc338ababe0b107af97eb567f`; do not use a seat's active clone.
2. Create one `mktemp -d` root outside every repository and deploy tree. Put the
   probe clone, sources, binaries, captured output, and `CARGO_TARGET_DIR` under
   that root.
3. Record exact host identity, kernel release, installed Landlock-header
   provenance, listeners, matching processes, both clone statuses, and the exact
   base before any probe.
4. Stop if either clone is dirty, the exact base is unavailable, or the scratch
   root is not the directory created for this run.

Illustrative setup, to be instantiated only after G1 approval:

```bash
BASE=0a30798a58349e1fc338ababe0b107af97eb567f
PROBE_ROOT="$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/beacon-g1-runio.XXXXXX")"
PROBE_CLONE="$PROBE_ROOT/beacon"
SCRATCH="$PROBE_ROOT/scratch"
mkdir -m 700 "$SCRATCH"
git clone --no-checkout <reviewed-local-beacon-clone> "$PROBE_CLONE"
git -C "$PROBE_CLONE" checkout --detach "$BASE"
test "$(git -C "$PROBE_CLONE" rev-parse HEAD)" = "$BASE"
test -z "$(git -C "$PROBE_CLONE" status --porcelain)"
export CARGO_TARGET_DIR="$SCRATCH/target"
uname -r >"$SCRATCH/pre-kernel.txt"
ss -Hltn >"$SCRATCH/pre-listeners.txt"
pgrep -af 'beacon-g1-runio' >"$SCRATCH/pre-procs.txt" || true
```

### Invariants binding every future approved probe

1. Loopback only: bind only `127.0.0.1` on ephemeral ports; no DNS,
   `0.0.0.0`, tailnet address, or off-host packet.
2. Scratch only: all created files and build output remain under `PROBE_ROOT`.
3. No Beacon source mutation: standalone replicas use the pinned sources as
   read-only references; both Git worktrees remain clean.
4. Reap every child with a deadline; leave no process group or listener.
5. Use deterministic barriers, never sleeps, for ordering.
6. Record tri-state results and raw errors. Absent, denied, and broken remain
   different outcomes.
7. Run each claim once. A surprising result is evidence, not a reason to retry.
8. Compare every result with the prediction written below before execution.

### P0 — exact host and ABI identification

Contributes one exact-host row to U1 and U2. It does not settle a target host
class, define the required host matrix, or establish anything about an AppImage
artifact.

```bash
uname -a | tee "$SCRATCH/host-uname.txt"
cat /etc/os-release | tee "$SCRATCH/host-os-release.txt"
stat -c '%n size=%s mtime=%y' /usr/include/linux/landlock.h \
  | tee "$SCRATCH/landlock-header-stat.txt"
sha256sum /usr/include/linux/landlock.h \
  | tee "$SCRATCH/landlock-header-sha256.txt"
grep -nE 'LANDLOCK_ACCESS_(FS|NET|SCOPE)_' /usr/include/linux/landlock.h \
  | tee "$SCRATCH/landlock-header-rights.txt"
cat >"$SCRATCH/abi.c" <<'ABI_EOF'
#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
int main(void) {
  errno = 0;
  long value = syscall(SYS_landlock_create_ruleset, NULL, (size_t)0, 1u);
  printf("abi=%ld errno=%d\n", value, errno);
  return value > 0 ? 0 : 1;
}
ABI_EOF
cc -o "$SCRATCH/abi" "$SCRATCH/abi.c" && "$SCRATCH/abi"
```

**[UNVERIFIED prediction]** One positive ABI integer is printed. The query uses
a null attribute and size zero, creates no ruleset, and owns no descriptor
(`landlock.rs:96-109`).

Pass for one host: positive integer plus captured host and header provenance.
The matching upstream ABI table, not omission from a possibly stale installed
header, determines which rights that ABI defines. A missing header, a header
that cannot be provenance-bound, or a header older than the reported ABI makes
the local-header portion inconclusive. Stop the complete packet on `ENOSYS`, a
non-positive result, ABI below 4, or an unclassified error.

### P1 — UDP under a production-shaped ruleset (design only)

Settles U3. A standalone scratch program reproduces
`landlock.rs:157-188` and `:236-257`: `handled_access_fs` equals the reviewed
mask, `handled_access_net` equals TCP bind plus connect, `/` receives read rights,
and only `$SCRATCH/inside` receives write rights. The parent binds one loopback
UDP socket and one loopback TCP listener. The restricted child sends one four-
byte datagram to the UDP port, attempts TCP connect to the listener, reports raw
errors in a fixed result record, and exits. The parent uses bounded receive/wait
deadlines and reaps it.

No executable command is specified: `p1.c` source bytes and their digest are
absent. They must be added and independently reviewed before a G1 approval
request can name P1.

**[UNVERIFIED prediction]** The loopback UDP datagram is delivered and the TCP
connect is denied with `EACCES`. One child is reaped; both sockets close.

- UDP delivered and TCP denied: pass for the predicted ruleset behavior.
- UDP absent and TCP denied: inconclusive; preserve raw results and stop.
- TCP succeeds: stop immediately because expected network enforcement failed.

### P2 — symlink asymmetry at the reviewed flags (design only)

Settles U5 at the flag-replica level. With a deterministic barrier, one half
opens a swapped target using `O_PATH | O_CLOEXEC` as in `landlock.rs:190-201`.
The other opens the root with `O_NOFOLLOW` and resolves the same target with
`RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS` as in
`worktree_launcher.rs:17-35`. All targets and effects remain under scratch.

No executable command is specified: `p2.c` source bytes and their digest are
absent. They must be added and independently reviewed before a G1 approval
request can name P2.

**[UNVERIFIED prediction]** The generic grant path follows the symlink while
the production-shaped resolver refuses it with `EXDEV` or `ELOOP`.

- Generic follows and production-shaped refuses: pass for the predicted
  flag-level asymmetry.
- Both refuse: partial; narrow ADR language to the observed result.
- Production-shaped follows: stop because containment did not hold.

This replica does not prove Beacon's full launcher. Calling
`run_bounded_for_test` is a separate G1 line item and is excluded here.

### P3 — socket failure versus enforcement (design only)

Settles U6. A standalone scratch program reproduces
`probe.rs:154-207` and `:220-278`. In its child, `RLIMIT_NOFILE` is lowered so
`socket()` returns `EMFILE`; the program preserves that raw error, feeds the
resulting bitset through a faithful `interpret()` replica, reports the variant,
and reaps the child. A seccomp-induced `EACCES` variant requires separate
approval and is not an automatic fallback.

No executable command is specified: `p3.c` source bytes and their digest are
absent. They must be added and independently reviewed before a G1 approval
request can name P3.

**[UNVERIFIED prediction]** `socket()` fails before any connection attempt,
`BIT_REFUSED_TCP` is set, and the interpretation reports `Enforcing`.

- `Enforcing`: pass; the candidate defect is confirmed for the replica.
- `FilesystemOnly` or `Unavailable`: pass; the candidate defect is absent in
  the replica and ADR language must be narrowed.
- Any other variant or unclassified error: stop and discard the interpretation.

### Cleanup and proof of cleanup

Run before removing scratch:

```bash
ss -Hltn | diff - "$SCRATCH/pre-listeners.txt"
pgrep -af 'beacon-g1-runio|/p1|/p2|/p3'
git -C "$PROBE_CLONE" status --porcelain
test "$(git -C "$PROBE_CLONE" rev-parse HEAD)" = "$BASE"
```

Expected results are no new listener, no matching process, an empty Git status,
and the exact base. Cleanup may remove only the validated `mktemp` root. Before
removal, require both a `beacon-g1-runio.` basename and a parent equal to
`${XDG_RUNTIME_DIR:-/tmp}`; otherwise stop and leave it for manual inspection.

### Stop conditions

Return to Kevin without continuing if:

- P0 finds no Landlock ABI or one below 4;
- TCP connect succeeds in P1;
- production-shaped resolution follows in P2;
- an error falls outside the pre-stated set;
- a child misses its deadline or survives cleanup;
- a new listener remains;
- either clone becomes dirty or leaves the exact base;
- any packet leaves loopback; or
- the work tempts a scope expansion, including U4 or Beacon's real launcher.

### Explicit exclusions

No real network destination, live service, deploy tree, fleet repository,
minimum-read-set experiment, Beacon launcher call, production source change, or
`scripts/gate` run is part of P0-P3. `scripts/gate rust` and `scripts/gate all`
run sandbox tests that reach `detect()`, so permission for that gate must be
stated explicitly rather than inferred from permission to edit documentation.

## G2 boundary

Even after G1, the following remain closed until Kevin accepts the exact ADR
0035 commit and separately authorizes implementation:

1. narrowing `/` read authority or changing the read observation;
2. changing the handled filesystem-rights mask;
3. adding UDP or named-destination enforcement;
4. changing generic grant symlink behavior;
5. changing socket-failure classification;
6. adding or changing a capability variant or serialized field;
7. changing a Receipt gap, conditionality, nested meaning, or version; and
8. exposing Declared Check execution through a production adapter; and
9. adding or changing a launcher, descendant-inheritance boundary, inherited
   environment, or working-directory authority path.

ADR acceptance, a Blueprint amendment, Receipt v3, live migration, and any
execution exposure are Product Steward decisions, not conclusions of this
static record.

## Reproduction without runtime effects

The static record can be checked without executing Beacon:

```bash
BASE=0a30798a58349e1fc338ababe0b107af97eb567f
git rev-parse "$BASE"
git rev-parse "$BASE:crates/beacond/src/sandbox/landlock.rs"
git rev-parse "$BASE:crates/beacond/src/sandbox/probe.rs"
git rev-parse "$BASE:crates/beacond/src/sandbox/mod.rs"
git grep -n 'grant(Path::new("/"), READ_RIGHTS)' "$BASE" -- crates/beacond/src/sandbox/landlock.rs
git grep -n 'LANDLOCK_ACCESS_NET_.*TCP' "$BASE" -- crates/beacond/src/sandbox/landlock.rs
git grep -n 'UDP egress' "$BASE" -- crates/beacond/src/declared_checks.rs crates/beacond/src/check_preflight.rs
git grep -n 'libc::socket' "$BASE" -- crates/beacond/src/sandbox/probe.rs
git grep -n 'preflight_project_checks' "$BASE" -- apps/desktop/src-tauri/src/lib.rs crates/beacond/src/local_transport.rs
```

These commands establish only repository contents. They do not establish host
behavior, execute a test, or cross G1.

[landlock]: https://www.kernel.org/doc/html/latest/userspace-api/landlock.html
