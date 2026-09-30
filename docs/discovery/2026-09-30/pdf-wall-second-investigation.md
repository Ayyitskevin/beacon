# PDF wall-second witness — bounded investigation

**A conditional stalled-clock risk was demonstrated in the actual PDF test path. No naturally occurring host clock stall was observed, and no remedy is proposed or implemented here.** This is the documentary result of joint overnight r2 B-R2-1; independent review and validation live in the accompanying workspace packet. It does not certify the full product suite or authorize landing.

## Question and source identity

The [landed clock audit](../2026-09-29/clock-fixture-audit.md) identified a source-level risk: `crates/beacond/tests/outcome_pdf.rs:181–200` calls its wall-second wait “bounded by construction”, but the equality loop has no elapsed-time limit if its observed second stops changing. The baseline here is `d52816c92734a603c9b42ea49349a5b2467958e4`. [Source pins](pdf-wall-second-sources.json) bind the repository contract (P01), original test (P02), unchanged renderer (P03), prior audit (P04), and workspace observations and artifact identity (W01–W13). `root=workspace` resolves from `./private-evidence`; these are retained local evidence files, not Git-hosted sources.

The original test invokes `helper_child_render` twice via its own executable, reads each PDF and compares nonempty bytes. This separation and the distinct-wall-second condition are important: a same-second comparison can miss a second-resolution timestamp leak. A backward change to a *different* second exits this equality loop; it is not the stall demonstrated here.

## Diagnostic instrumentation, not a source-test candidate

Sol preserved the original test and renderer, then instrumented only the test in an isolated worktree (`beacon-joint-overnight-r2-20260930`, base above). The exact diff is W03. It retains the same parent loop, helper process, fixture, real renderer and original byte assertions. Extra logs identify parent/child PIDs and actual render-start wall seconds. A test-local `now_secs()` seam holds one observed value only in synthetic `stall` mode; helper logging continues to read real wall time. The selected test's directory removal is replaced with retention. No production clock input, global fake clock, host time change or renderer modification was introduced.

W04 is the separate supervisory script used by the author. For this stalled run it waits for the parent's actual wait-entry log, observes eight monotonic seconds, and terminates its own test parent. That bound belongs to the diagnostic supervisor, not the original test. An overall setup/build timeout is separately labelled and would not prove the defect. All fixtures and logs were retained. Only the exact named parent and its helper ran; other tests, including a separate destructive cleanup path, were not selected.

## Observations

| Case | Direct observation | Interpretation |
|---|---|---|
| Offline locked build (W05–W06) | `cargo test --offline --locked -p beacond --test outcome_pdf --no-run -j 2 --target-dir <owned target> --message-format=json-render-diagnostics`: exit 0 in 104.23 s | Diagnostic artifact built locally. No test outcome follows from the build alone. |
| Normal-clock control (W07–W08) | Test exit 0 in 1.94 s; child PIDs 464002 and 464037; actual render-start seconds 1790742640 and 1790742642; wait exited after 804 ms | Real separate-process/different-second path and original assertions completed. |
| Synthetic constant observed second (W09–W10) | First child PID 464600 rendered and exited 0. Parent PID 464598 remained in the wait; 160 polls and 8025 ms recorded. Actual wall second advanced 1790742663→1790742671 while the injected observed second stayed 1790742663. No second-child spawn or `b.pdf`. Supervisor exit 124; owned parent exit −15 | The conditional lack of a bound is observed in the real parent path, not inferred from a toy loop or from an unrelated timeout. This is an intentionally stopped diagnostic, not a passing regression test. |

Normal `a.pdf` and `b.pdf`, and the stalled run's `a.pdf`, are each 47,132 bytes with SHA-256 `17fb931eab2726c3114ff865d94d2e3699502944728b731ba0e3e7fb53f86d72`. Exact retained paths, all events and timings are in W07/W09; raw logs are W08/W10. The real host clock advanced throughout the synthetic observation.

## Reproduction and limits

The workspace packet is `reports/beacon-joint-overnight-r2-20260930/`. Its `sol/` directory retains baseline bytes, instrumentation diff, supervisor, command records, executable and observations. Start by checking the source-pin hashes and inspecting the diff and supervisor. Use fresh retained output directories; do not rerun the original unmodified test cleanup or a broad suite under this run's no-deletion fence. Exact build arguments are W05; each normal/stall run has its own `COMMAND.json` beside W07/W09. W11 pins the 600,769,808-byte diagnostic executable (SHA-256 `dd704e8f3885f0aec219d7a0da421792f06c61ce058f2709802bbd04ed9c753d`) and instrumented test bytes. A source or binary mismatch invalidates the corresponding runtime claim.

**Preservation/write-fence attestation is partial.** W12 records the pre-existing `~/.cargo/.global-cache` modification time at 00:28:01.745085028 EDT, inside the build window. No pre-build snapshot exists; neither write authorship nor absence of implicit Cargo metadata writes outside the task directories can be certified. No cache repair, deletion or second build was attempted. W13 freezes the author report and raw artifacts. This limitation does not establish a deletion or invalidate the observed render/wait outcomes, and is not a waiver of the agreed fences.

This result establishes what happens **if the test's observed second stalls**. It does not establish the frequency or cause of a real clock stall, a production renderer defect, expiry behavior elsewhere, or a tested fix. No test-source change is committed by this documentary branch. The synthetic seam, timing logs and retained cleanup are scratch instrumentation only. B-ON3 remains parked under the agreed H2-A1 boundary; remedy design or a source-test candidate requires a subsequent jointly agreed amendment. M6 ratification, Q01–Q06 authentic returns, B1/M1, B7, G3/capacity and P1-Muse-first remain untouched. Muse's same-flow exact-head review remains required for any future landing; nothing was pushed or run hosted.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
