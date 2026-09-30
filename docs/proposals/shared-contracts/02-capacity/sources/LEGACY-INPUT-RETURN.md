# Capacity02 input L — conductor source-recipe return

**PRAGMATIC PARTIAL: source pins and exact existing case recipes supplied; no fixture bytes generated, tests run, or pilot admission.** Prepared Sep23 by Codex helm, session `01a0b9ab-33bc-7920-b40c-7bcef4282311`. Independent review pending at freeze.

## Why this is next, not repeated design work

`reports/beacon-overnight-v2-20260917/a2-cap-a-design/INPUTS-NEEDED.md:13` assigns legacy input L to the conductor: exact dispatch/checkpoint fixtures, including malformed, overlap, wrong-head and prior-unresolved cases. The earlier `SOURCES.sha256` already pins files; it does not map these cases to their constructor helpers and literal assertions. This return supplies that mapping without regenerating completed contract proposals or adding new acceptance requirements.

The five existing code files are read-only. Current hashes for dispatch/checkpoint tests and both public readers match the earlier design's four corresponding pins. `test_packet.py` and the external `seat-report` parser are additionally bound here so readers can follow the actual fixture/helper chain. This is a source snapshot, not a claim that a future execution environment or generated fixture bundle is reproducible from hashes alone.

## Fixture recipes and ownership

Paths below are relative to `./private-evidence`; full-file hashes are in `SOURCE-SHA256SUMS` (verify from that cwd). No test module was imported or executed for this return.

- Dispatch: `conductor-tools/test_dispatch.py:15–85`. `setUp` owns a temporary directory and counter; `artifact`, `events`, `dispatch_record`, and `save` generate `dispatch.json`, brief and witness files with content digests. `cli` invokes the public `dispatch_check.py` with that record and temporary cwd. `assert_refused` means exactly returncode 1 and stdout `""`; more specific error text is asserted only where stated below. Fixture command-like strings are inert source text, not instructions or actual seat observations.
- Checkpoint: `conductor-tools/test_checkpoint.py:11–112` delegates base setup to `test_packet.PacketTests`; `write_checkpoint`/`save_checkpoint` generate the checkpoint sidecar and digest binding, while `previous_checkpoint` generates the predecessor record. `conductor-tools/test_packet.py:14–74` supplies synthetic HEAD/REPAIR, isolated packet/empty corrections registry, artifact writer and the public `packet.py --registry <isolated-registry> <packet> view checkpoint` invocation. Names such as helm/blue/security and text such as `22 tests passed` are synthetic fixture literals, not real identities, security review or runtime outcomes.
- Predecessor witness paths/hashes are reference-shaped synthetic values, not materialized historical files: `test_checkpoint.py:99–107`. `packet.py:374–389` parses the predecessor with `materialize=False`; current evidence uses `materialize=True` at365–367. Do not claim this fixture authenticates previous custody.
- The supplemental completed-gate control uses `test_packet.py:90–113`; `packet.py:236,253` references `/example/checkouts/shared/bin/seat-report`. That read-only parser is separately hash-bound. The selected test setup passes its own temporary registry; the live `conductor-tools/corrections.json` is not an input to these recipes.

## Exact existing case map — every row NOT RUN in this slice

Each method below belongs to the named class in the pinned file. Line ranges bind the recipe and assertions; they are not completed checks.

| L category | Existing recipe / method | Literal source expectation and scope limit |
|---|---|---|
| Malformed dispatch | `test_dispatch.py:209–244`, `DispatchCheckTests.test_malformed_records_artifacts_and_caps_refuse_with_empty_stdout` | Caps `True, 0, 33`; duplicate record; state `idle`; missing/escaping brief; missing/empty witness use `assert_refused` (exit1/empty stdout). Only duplicate/state/empty witness add their named stderr substring. Mutated witness separately asserts exit1/empty stdout/`hash mismatch`. These are legacy fixture limits, not approved CAP-A bounds. |
| Pending capacity / overlap | `test_dispatch.py:140–168`, `DispatchCheckTests.test_pending_records_reserve_capacity_and_substitutes_refuse` | Two variants (`blocked`, `withdrawal-requested`) plus substitute at cap1: exit1/empty stdout and `exceed worker cap`. Two active same-lane records: `lane overlap`; two active same-seat records: `seat overlap`, each via `assert_refused`. This is recorded worker accounting, not compiler occupancy or live-seat determination. |
| Existing release control | `test_dispatch.py:170–191`, `DispatchCheckTests.test_acknowledged_withdrawal_or_terminal_observation_frees_capacity` | Two existing terminal-history variants, each followed by substitute at cap1: exit0; `reserved=1`, `remaining=0`, two records, old reserves false/new true. No authentic withdrawal instruction or current capacity follows from these synthetic strings. |
| Missing checkpoint | `test_checkpoint.py:154–162`, `CheckpointTests.test_legacy_packet_has_no_checkpoint_claim_and_checkpoint_view_refuses` | Legacy JSON exits0 and lacks `checkpoint`; checkpoint view exits1, stdout empty, stderr contains `checkpoint sidecar absent`. Optional-sidecar behavior is not implemented by CAP-A. |
| Wrong head + malformed checkpoint | `test_checkpoint.py:231–255`, `CheckpointTests.test_missing_or_malformed_checkpoint_evidence_refuses` | Change checkpoint head to REPAIR while selected round remains base; blank SKIP reason; missing PASS witness; mutate tooling evidence; malformed predecessor. **Each asserts only returncode1.** No stderr/empty-stdout assertion is supplied by this method. `packet.py:368` separately names `checkpoint head mismatch` in source; it is not a measured diagnostic here. |
| Prior unresolved identity | `test_checkpoint.py:164–208`, `CheckpointTests.test_repair_round_carries_predecessor_unresolved_identity` | Round1 carries predecessor `broad-review/security/Complete broad independent review` with new synthetic PASS witness: exit0 and `security: PASS`. Three negatives replace the row (`different-row`, not an empty obligations list), alter scope, or alter text: exit1/empty stdout and regex `omitted\|relabeled`. This test uses a prior SKIP row, not a prior FAIL row; the broader source branch handles both at `packet.py:382`. |
| Missing predecessor | `test_checkpoint.py:257–266`, `CheckpointTests.test_repair_round_requires_actual_previous_checkpoint_artifact` | Selected repair round with previous null: exit1/empty stdout and `requires previous checkpoint`. No predecessor is fabricated to make the repair valid. |
| Scoped verdict / publication control | `test_checkpoint.py:114–152`, `CheckpointTests.test_scoped_checkpoint_is_distinct_from_gate_review_and_publication` | Checkpoint view exit0; includes `Checkpoint verdict: PARTIAL`, tooling PASS, product/security SKIP, gate PASS, content PASS and `Publication: MUSE-ONLY / UNKNOWN`; excludes product-ready/merge-ready/standalone GO. Five propagated views assert exit0 and claim-scope text. Synthetic gate PASS never implies real acceptance. |

Load-bearing source check: `test_checkpoint.py:185–208` really substitutes `different-row` for its empty-list mutation and asserts `omitted|relabeled`; `packet.py:380–389` compares prior unresolved ID/scope/text. The map does not turn the empty Python list into an empty JSON obligations fixture or assert a test for predecessor FAIL that is not present.

## What this does and does not return

- Supplied: exact named existing recipes, complete relevant source-file pins, helper links and literal expected results. No new requirements, fixtures, runner or implementation.
- L remains **PARTIAL / source-reference return** until the appropriate owner accepts this documentary input. If a later admission needs immutable generated fixture bytes, its author must materialize and freeze them only inside separately admitted S/B limits, with a non-author reviewer. This document grants none of that work.
- `ACCEPTANCE.md:56` CAPA.14 also includes earlier-conflict/later-agreement history, divergent bytes for one admitted identity, and delivery/baseline-as-capacity-proof cases. This legacy mapping does **not** satisfy those CAP-A comparison subcases or CAPA.07 compiler-conflict behavior. They still depend on S/B and applicable R/C/T/O inputs; no new test coverage is claimed.
- All S/R/C/T/O/B, privacy/pilot/owner gates remain unresolved by this return. Existing P1, D4/D7, B7, B1/S3 and E14+ restrictions remain. B9 has no demonstrated dependency here and stays parked. No Jev/provider calls.
- Delivery03 Option A is orthogonal: its retired runner is neither executed nor reused, repaired, copied as a replacement, or cited as a complete failure collector. Historical successful packets and the open P2 remain intact.

## Verification and resume binding

Read-only verification command (Codex cwd): `sha256sum -c reports/beacon-contract-continuation-20260923/capacity-l-author/SOURCE-SHA256SUMS`. Nine source entries were obtained by direct hashing; freeze verification is recorded in the run ledger. `FROZEN.sha256` is checked from this author directory and binds this return plus its source manifest, never itself.

Independent review must test accuracy and scope separately, re-read one critical literal assertion, verify hashes, and report gaps without editing this packet. No runtime/test correctness PASS is requested. On hash drift, stop and report; do not update pins to conceal it.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
