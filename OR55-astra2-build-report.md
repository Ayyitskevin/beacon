# OR55 astra2 build report

task: R55 — non-zero silent Git exits are not absence witnesses.
stage: codex / astra2 / w7:pF
session: 2026-09-07
lane: fleet-handoff
produced: R55 implementation and tests in this own worktree; one local commit, no push.
verified: Pending implementation; commands and observations are recorded below.
assumptions: Base is explicitly pinned to 56659aea despite origin/main advancing to ece38e50 after fetch. Nothing merges before Kevin's morning board call.
open: Reviewer sol1 or grok; gate requires FF-69 symlinked venv.

## State table (written first)

| exit | stderr | site witness | state |
|---|---|---|---|
| 0 | empty / absence-witness / other | positive / silence | `Bytes(stdout)`; success owns the output |
| non-zero | empty | positive | `NotFound` only when the site-specific positive witness is present; Silence has no positive witness |
| non-zero | absence-witness | positive | `NotFound` |
| non-zero | other | positive | `Unreadable(ReadRefused { message })` |
| non-zero | empty | silence | `Unreadable(SilentExit { code })` |
| non-zero | absence-witness | silence | `Unreadable(SilentExit { code })` unless a future site supplies a distinct positive witness |
| non-zero | other | silence | `Unreadable(ReadRefused { message })` |
| signal / timeout / spawn failure | any | any | existing typed unreadable failure |

The R55 implementation keeps `AbsenceWitness::Message` for `cat-file`, where Git
positively names the missing path. Every `AbsenceWitness::Silence` non-zero exit is
now `SilentExit { code }`; no caller may infer absence from that silence.

## Measurements before implementation

Contradiction observed before acting: after the required fetch, `origin/main` was
`ece38e50` (R51), not the brief's `56659ae`; I recreated the own branch at exact
`56659aea1a39b970484d865bb787c21b8f213613`.

On a real temporary repository, `git merge-base --is-ancestor` returned exit 1 with
empty stdout/stderr for a non-ancestor; `git config --includes --name-only --null
--list` returned exit 0 with empty output for no entries. Thus the former's negative
answer is not a positive absence witness; the latter is success, not a silent
non-zero case. `git cat-file blob HEAD:missing` returned non-zero with stderr naming
`does not exist in`, the positive message witness retained by R46.3.

## Implementation and tests

Implemented `PinnedReadFailure::SilentExit { code }` and added its serialized name; all
`AbsenceWitness::Silence` non-zero exits now fail closed. The exact sol1 regression is
`worktree::r46_read_classification::sol1_exit_128_with_empty_stderr_is_not_an_absence_at_every_silent_site`.

Verification: `cargo test -p beacond --lib worktree::r46_read_classification -- --nocapture`
passed 14/14, and the legitimate NotFound test
`worktree::r46_read_classification::a_non_zero_exit_in_a_pinned_repository_is_not_found_and_never_unreadable`
passed. The exact sol1 command passed 1/1. I temporarily restored the old
`Silence => NotFound` branch; the sol1 command then failed (expected RED), and restored
the implementation; rerun passed (GREEN).

<!-- seat-report:v1 -->
HEAD: ba122bb44632f320e45389f6327d1f36ae0a928a
GATE: {"status":"FAIL","exit":2,"log":"/example/checkouts/beacon-astra2-r55/gate-2aa015a.log","reason":"gate requires a local .venv; beacon-review3/.venv is absent"}
VERDICT: HOLD
PROBES: {"pass":16,"fail":0,"skip":1}
RESTORED: YES
<!-- /seat-report -->


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
