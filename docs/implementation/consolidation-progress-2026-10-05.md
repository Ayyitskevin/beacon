# Beacon consolidation progress — 2026-10-05

This is a feature-branch progress publication, not landing, production approval, or completion of
all three consolidation milestones. The source candidate is
`335c5be3b1f419bb782baee3c9571dcb73c25c89`; the publication adds only this note.
Kevin authorized this GitHub sync through request `astra-github-sync-20261006-1791247382084`.
Muse retains exact-head review and the merge lane. No product tests were rerun for publication.

## Milestone state

| Milestone | Current evidence | Remaining work |
| --- | --- | --- |
| M1: six origins become one buildable native codebase | A native pure Evidence verifier is implemented and independently tested; the integrated branch also contains a retained native workflow witness and a scoped verification entry point. | Conversation owner interfaces and the native Claim/Evidence/Finding owner chain are not completed by this helper. Full admitted module connectivity/build acceptance remains open. |
| M2: one useful integrated workflow | **One admitted native execution PASS** on the exact source candidate: synthetic Project/Team/Mission → stored completion → canonical Outcome Candidate → same-store restart and authenticated read-back. | Muse's source/evidence acceptance remains separate. This one case does not establish every workflow acceptance row, six-capability completion, or production readiness. |
| M3: reproducible, handoff-ready tree | An independent clean checkout reproduced the selected offline recipe; the executed workflow has retained, independently checked evidence. | Full applicable contributor gates and final integrated acceptance remain incomplete. Installed-toolchain/cache prerequisites and held effects are explicit; selected checks are not full CI. |

The Oct 5 compile-only checkpoint is historical. The later owner-admitted native run below is the
current M2 evidence. This note does not amend historical receipts or expand the verification script:
that script still compiles `local_transport` only and never executes it.

## Source lineage and module boundaries

GitHub base: `c3b9d7024b5f00f05f55e96cc70768383069fc9d`.
Evidence implementation/review head: `c853c5e7719166a5c34dd039006b072cdcaf429c`.
Integrated source/review/runtime head: `335c5be3b1f419bb782baee3c9571dcb73c25c89`.
The Evidence head is an ancestor of the integrated head, not a separate unintegrated dependency.

These six inherited source pins come from the consolidation source census and existing domain docs.
They are historical inputs, **not current upstream-head claims or six completed integrations**:

| Origin | Inherited source commit | Native boundary |
| --- | --- | --- |
| Ayyitskevin/athena | `8ddefc8def03c518811f118a7404625bb1155b62` | Work / Project, Team, Mission |
| block/buzz | `78cbffeb64c01220e705adf0aa9690fdbd0d7a37` | Conversation / Thread |
| Ayyitskevin/Minerva | `9968a5ef21a1938067bc7826b5fa1977c6d9d574` | Evidence / Knowledge |
| Ayyitskevin/Vulcan | `56ba96e25ad07b49f85c95d837bd08186fd3c40a` | Inference |
| Ayyitskevin/Janus | `6b62110e9c918c67219ab0b45dbf4dd9937d6620` | Authority / Proposal and Ruling |
| Ayyitskevin/Icarus | `c919cd2f9dd834de22cb8c89efb8e4f629411742` | Execution / Evaluation |

See [ADR 0002](../adr/0002-unified-agent-office-suite.md) and the
[scoped verification recipe](consolidation-core-verification.md). ReplayDeck is native design,
not a seventh imported tree. This publication imports no additional legacy delta.

Completed source changes are limited to `crates/beacond/src/evidence.rs`, its exports in `lib.rs`,
`tests/evidence_span.rs`, retained evidence in `tests/local_transport.rs`,
`scripts/verify-consolidation-core`, and its documentation. The pure helper binds bounded complete
snapshot digests, UTF-8 byte ranges and exact quotes. It establishes no truth, authorship, permission,
Evidence admission, storage, or native Knowledge owner chain.

## Verification actually completed

| Subject | Actual result | Independent review |
| --- | --- | --- |
| Evidence head `c853c5e7…` | 34/34 pure tests passed in both author/integrator and independent runs; focused Clippy with warnings denied and scoped rustfmt passed. | Sol High correctness PASS; Daybreak security PASS; zero open P1/P2. Final delta reviews carry their explicitly identified predecessor reviews. |
| Integrated head `335c5be3…` | Clean-checkout recipe: 34/34 pure tests, 12/12 benign CLI refusal controls, `local_transport --no-run` compilation; all 15 recorded wrapper steps exited 0. 104.751 s, maximum RSS 2,404,416 KiB. | Sol High correctness and Daybreak security PASS-WITH-NOTES; zero open P1/P2. |
| Same integrated head, native workflow | One attempt; **1 passed, 0 failed, 0 ignored, 34 filtered**; tool and command exit 0. Monotonic elapsed 1.842569632 s, maximum RSS 61,260 KiB. | Both recipe reviews PASS, then non-author correctness execution/evidence and independent security evidence PASS-WITH-NOTES; zero open P1/P2. |

Astra authored the verifier/workflow changes; supporting Sol authored regressions/verification
entry point; Helm integrated. Correctness was the distinct `gpt6-blue-validator` seat (Sol High),
security the `gpt6-blue-reviewer` seat (Daybreak). Authors did not review their own changes.
Roster labels describe assignments, not an independent model-runtime attestation.

Review progress is retained: the first round-2 integration had an unavailable enum reference and
permissive receipt modes (two P2s). The final source preserves the assertion via serialization,
creates/verifies owner-only receipts, and fixes duplicate-option handling. Both deltas closed.
A reproduction read-back initially expected 16 steps instead of 15; the corrected ordered-list
check passed without another Cargo run. Earlier failures remain in local evidence.

The native case was `the_command_journey_retains_completion_and_candidate_bytes_across_server_restart`.
Kevin's Oct 5 admission covered freshly owned socket unlink after owner/type checks, test SQLite
bootstrap/migrations/WAL, synthetic account/policy/data setup, and loopback metadata HTTP only.
The run reused the compiled binary without rebuilding. Six metadata requests received six responses;
canonical candidate bytes and bindings survived restart, locked access preceded re-authentication,
and original records were read before repeat preparation. Three domain counts stayed `[1,1,12]`;
publication count stayed zero. Candidate Markdown was 6,639 bytes, raw SHA-256
`af2d54d2112978a1d278e5ce648da42cc698fd9b558ad71e263e0d909af08b1c`.

The external 120-second ordinary KILL watchdog had no grace or retry. This is **not** a physical-exit
guarantee, generic stale-socket safety proof, OS resource quota, or G3 approval. The test's own
`NOT_ESTABLISHED` whole-journey deadline record remains intact. Recorded storage accounting was
909,743 bytes, below the admitted 256 MiB; accounting is not enforcement. Exact source/binary and
named receipts are bound; whole-cache/target immutability is not claimed. An early missing-activation
refusal and later pre-shell sandbox initialization failure produced zero native attempts; the single
actual native execution passed. No later rerun is implied or authorized by this publication.

## Documentary dependencies and preserved evidence

Conversation B/O/E are settled technical decisions; V/T are delegated policy decisions: whole
Thread projection or refusal, authorized typed references including Proposal, and no embedded target
bodies or implicit reads. The completed Conversation implementation-design packet has independent
correctness PASS-WITH-NOTES and security PASS, zero open P1/P2; its nine behavioral witnesses are
UNRUN. Supervisor resolver, pure-expiry, Q2 and record/Event-owner interfaces remain prerequisites.
No new policy answer is requested where one is already recorded.

Native Evidence's gap map, C0–C5 consumer sequence and ten future witness pairs are independently
reviewed documentary work. The later tested pure helper is preparatory; it does not complete those
owner interfaces or run the wider ten-pair matrix. The reviewed workflow admission design led to the
separately admitted run above; that run does not claim all eight original acceptance rows passed.
Muse acceptance of these predecessor/documentary packets has **not** been inferred.

Raw evidence stays in the fleet's private review archive. These digests identify exact manifests for
Muse's existing review packets; their contents are not uploaded with this PR:

| Evidence packet | Manifest SHA-256 |
| --- | --- |
| Evidence implementation, 23 entries | `5bec4238e7f01eab90ff4106ab9ffc4db4d63859aa30d1987112a93b0a8c8f80` |
| Round-2 final packet, 29 entries | `13ab34de68937a631aef14c6809697d5a309aaab584c72755b83faabfb9efe87` |
| Round-2 reproduction, 141 entries | `e988978a386fe536d7351de21f9644fa5bf330af24caf1487f79450fd9474cee` |
| Native M2 execution packet, 45 entries | `d39f20825c828091ed3520699107fadc0d9db28924a51c8df3ea460a615ab55f` |
| Native M2 run receipts, 28 entries | `67cb144244d01d37ef38ef109fdd69017a9038753ef66c062884a6f83a0e70a8` |

No credentials, synthetic token files, databases, raw host logs, private fixture paths, caches, or
customer data accompany this publication. The local artifact custody and full prior verdicts remain
available to Muse; a digest table alone is not independently accessible execution evidence.

## Exact next gates

1. Muse reviews the final publication head plus the existing Evidence, round-2 and native M2 source/
   evidence packets. Internal PASS is not Muse acceptance, a main push, or permission to merge.
2. Finish the applicable native owner interfaces and admitted module connections for M1. Settled
   Conversation policy is not a substitute for those interfaces or authority to release Q2 holds.
3. For full M3, scope and admit the exact remaining contributor-gate effects: Rust workspace/PDF
   and cleanup families, desktop npm/Tauri/sidecar build and log teardown. Obtain any required narrow
   owner grants; do not request or infer a blanket hold lift. Then reproduce the applicable final
   checks and workflow on a clean checkout with declared prerequisites, and independently review
   the final tree. No fresh-machine acceptance condition is added by this note.

Full workspace tests/lint/build, desktop and PDF checks, provider/model runs, real-record processing,
independent Evaluation, Delivery execution and Par wiring were not established by this checkpoint.
Hosted Actions remain unavailable by policy; publication uses `[skip ci]` without changing workflow
configuration. Pending/skipped checks are not PASS and this is not a required-check waiver.

Delivery Item 2 remains **HOLD / NOT_ADMITTED / UNRUN / zero attempts** (ENV-PHYSICAL-EXIT-01).
B-ON3 execution/cleanup/Cargo isolation, B1/M1, B7, G3/capacity, P1-Muse-first, Q2 A3/C1 and the
dormant Par hook retain their existing holds. No authority, permission or Tier change, deletion,
production deployment, main push or landing accompanies this sync.
