# Sidecar01 R09 — conditional future fence and review proposal

**DRAFT / NOT A BUILD GRANT.** Conductor-owned R09 contribution only. R01/R02/R03/R08 and all five Sidecar P1 holds remain unresolved; no module, tests, fixtures, directory or branch is created by this proposal. Draft author: Codex helm, September25,2026. Independent documentary review required before the Muse gate.

## Why this is genuine remaining work

[OWNER-DECISION-PREP.md:121–134](../beacon-e01-e10-discovery-20260921/OWNER-DECISION-PREP.md) asks the conductor for a future fence, source/regression pins, verification commands and a real non-author reviewer before Muse's separate build decision. Existing E01–E10 discovery supplies selectors/pins, not that future fence or reviewer assignment. This draft supplies the missing proposal without regenerating the interface, experiments, owner questionnaire or final profile.

The initial intake's proposed stop was too restrictive. Blue's PARTIAL review and root source verification are retained in [INTAKE-REVIEW-RETURN.json](INTAKE-REVIEW-RETURN.json) and [R09-SELECTION-REPLAN.md](R09-SELECTION-REPLAN.md). This draft does not convert the intake to an unqualified PASS or close any owner row.

## Fixed source contract; unresolved configuration stays unresolved

Use the existing [Sidecar01 proposal](../beacon-weekend-v2-20260919/contract-proposals/01-EVIDENCE-SIDECAR.md), especially interface:24–44, non-effects:75–90, E01–E10:101–110 and owners:118–123. Optional-local B5-A only; mirror compatibility NOT ASSESSED. This is not a new interface, shared schema or policy.

The future module would consume explicit pinned packet/selected round, sole correction registry and optional sidecar under an owner-approved local profile. It would return evidence support/uncertainty, never publication/readiness/authority. It must reuse the existing confined regular-file/digest reader, preserve legacy refusal semantics, and treat command/grant text as data.

| Input still absent | Supplier / required later binding | Consequence before implementation |
|---|---|---|
| R01 exact format/version/selectors, typed comparison and count/runner meanings | Muse semantics plus source-owner field evidence | No default profile, enum extension, parser adapter or selected subset silently invented. |
| R02 exact allowed diagnostics/labels/quotes and custody references | Muse disclosure plus source/run owners | No sanitizer or source-label policy designed; no private payload or B7 work. |
| R03 selected run/round, packet/registry/witness pins and authenticated custody | Source/run owners through existing approved channel | No real run selected or copied merely to fill the slot. Synthetic fixtures are not owner evidence. |
| R08 finite limits/units/applicability and actual delegation | Muse/authorized admission owner | No numeric deployment/test ceiling chosen here; historical16MiB is not a default. |
| R09 source baseline, candidate fence/effects and actual build-time reviewer | Conductor proposes below; Muse separately disposes | Proposal may be revised/rejected. No assignment or build permission is inferred. |

These missing values do not block writing/reviewing THIS draft. They do block an execution-ready implementation admission. Muse can explicitly narrow supported scope later, but no silence becomes an implicit narrowing.

## Proposed isolated candidate fence — not established or writable now

Proposed root: `/example/checkouts/beacon-sidecar-r09-candidate/`. It was absent at proposal preparation. This names a future **own workspace**, not a claim that a clone/branch already exists. The inspected conductor-tools bundle is not a Git worktree; its actual file digests, not an invented commit, bind this proposal. If Muse requires a Git clone/ref, its exact source and creation authority must be named in the future grant. No conductor branch may be edited.

| Exact proposed path under that root | Future writer / allowed purpose |
|---|---|
| `candidate/evidence_sidecar.py` | One admitted implementation author; optional local checker only, matching the approved profile and existing contract. |
| `candidate/test_evidence_sidecar.py` | Same author; synthetic public-interface controls mapped to E01–E10, independent literal expectations; no tests of actual owner identities. |
| `admission/INPUT-BINDINGS.json` | Helm alone; exact owner-return source refs/digests, selected source snapshot, limits and allowed effects. Not a new authority schema or a grant on its own. |
| `admission/GRANT.md` | Helm alone; verbatim future authenticated Muse ruling, source provenance and exact admitted scope. Absent today. |
| `baseline/packet.py`, `baseline/test_packet.py`, `baseline/test_checkpoint.py`, `baseline/test_corrections.py` | Helm-only byte-identical source snapshot preparation IF separately permitted; read-only during author/reviewer work, pinned below. No patches or silent helper replacements. |
| `author-evidence/` | Author-only finite synthetic inputs/temp/results and complete command receipts within expressly admitted R08 bounds; exclusive new files, freeze before review. |
| `review-evidence/` | Non-author reviewer only, independently bounded fresh fixture/temp/result output, with same input pins; no writes to author files or baseline. |

The two directories are bounded trees only when the later grant supplies finite artifact/byte/count/depth/time limits and exact accepted contents; they are NOT wildcard permission today. A later grant must name a separately frozen candidate revision/digest set and source snapshot before review. Ordinary source dependency remains the pinned shared seat-report parser, read-only; relocating, rewriting or replacing it needs a revised reviewed proposal. The registry supplied through R03 remains the sole authority; an immutable copy, if explicitly allowed, is only a source-bound input, never a second correction registry.

No writes in the Beacon product checkout, live conductor-tools bundle, shared/bin, shared rules, old reports, current registry or another seat's workspace. No mandatory adoption, watcher, hooks, packet schema edits, publisher, Athena/Janus mutation, node-b integration, service, model, network or account operation. If reuse requires changing a shared reader, stop and submit a new exact proposal; do not widen this fence.

## Legacy source pins and preserved evidence

[R09-SOURCES.sha256](R09-SOURCES.sha256) binds the existing contract/owner request, experiment selector map and source baselines. Verify from Codex cwd. Key reader/test pins:

- packet.py: `5026a2374edf8618258c0e6433a2b8501c53debf51c5fefa21dffac3c039e344`.
- test_packet.py: `813decee6b78234f147dc53a5937fe396728f7a3da6441f3374a3961bdbc292c`.
- test_checkpoint.py: `e1089fa3c9108a79ad51428cd23455d17c7b1e926179b58cf9f6502e9f99d428`.
- test_corrections.py: `ac378ac2dbcab4db77a9ec94750c648d9189c39247feffdb71ea3501766f8c1c`.
- shared seat-report: `a3fbd5be004f9dfa97e03702a701c1f1120bb037bb7ec3f948701a04984b790e`.

Reuse [experiment.py:71–89](../beacon-e01-e10-discovery-20260921/experiment.py) for exact E01–E10 legacy selectors; the [pre-execution plan](../beacon-e01-e10-discovery-20260921/PLAN.md) already distinguishes observed legacy behavior from missing sidecar coverage. Its frozen E## packets/transcripts are historical evidence only. Do not rerun E01–E10 simply to reproduce their results. E02/E05/E06 expose missing typed/count/restoration semantics, not bugs this draft authorizes in the old parser. E07 obligations and E10 absence must retain their current public-reader meaning.

The future new-interface suite must cover the existing E01–E10 acceptance rows, including unsupported/missing-profile behavior and literal adversarial controls; **no coverage or PASS is claimed now**. Testing declared unsupported scope is not permission to implement new semantics.

## Proposed verification commands and effects — NOT RUN

These are candidate commands for a later exact grant, not commands to execute from this draft. Source checksum command alone is safe now:

```sh
sha256sum -c reports/beacon-resumption-20260925/R09-SOURCES.sha256
```

After actual future admission, finite test bounds, prepared snapshot and candidate exist, proposed new-interface command (no current implementation exists):

```sh
PYTHONDONTWRITEBYTECODE=1 TMPDIR=/example/checkouts/beacon-sidecar-r09-candidate/author-evidence/tmp python3 -B -m unittest discover -s /example/checkouts/beacon-sidecar-r09-candidate/candidate -p 'test_evidence_sidecar.py' -v
```

Proposed regression command, only after changed candidate and express permission for isolated regression execution—not an unchanged discovery rerun:

```sh
PYTHONDONTWRITEBYTECODE=1 TMPDIR=/example/checkouts/beacon-sidecar-r09-candidate/author-evidence/tmp python3 -B -m unittest discover -s /example/checkouts/beacon-sidecar-r09-candidate/baseline -p 'test_packet.py' -v
```

Repeat that exact pattern for the two explicit filenames `test_checkpoint.py` and `test_corrections.py`; no broad test discovery. Reviewer independently uses the same frozen candidate/sources and its separate `review-evidence/tmp` path, under its own admission. The grant must specify actual test wall-time/process/output limits; this draft chooses none. Inspect all test/helper effects before admission; setting TMPDIR is a proposed destination, not proof every write is confined. Refuse any test path that needs a write outside the grant.

Capture exact argv/cwd, environment names without secret values, wall clock, complete stdout/stderr, real process status and all intentionally skipped cases inside the admitted fence before assertions. The Delivery03 runner is not reused. Hash inputs before/after; prove expected refusal cases without converting loader refusal to semantic support. No arbitrary model judges, real data or product gate rerun. Runtime confinement, security adequacy and full failure-capture reliability remain unverified until separately admitted evidence establishes them.

## Actual documentary reviewer versus future appointment

**Proposed independent reviewer: Blue, w7:p7, session01a0b9ab-472d-7750-97f3-00095543df87.** Blue just reviewed the new intake (DONE221,1m56s) and settled20s; original session preserved. [REVIEW-START.json](REVIEW-START.json) and [INTAKE-REVIEW-RETURN.json](INTAKE-REVIEW-RETURN.json) bind actual current participation. Blue did not author this R09 draft. Helm will separately admit this frozen draft after a fresh composer/identity check; actual verdict/receipt will be appended as separate artifacts, not asserted in advance.

This is a named, currently observed candidate for documentary review, **not** permanent availability or implementation-review assignment. A future build needs a named actual author different from the reviewer, fresh availability/session evidence, no overlapping writes, a renewed claim and Muse's explicit fence. If Blue is unavailable then, request a separately approved non-author route; do not substitute silently. Historical experiment reviews alone would not meet this requirement.

## Muse disposition requested

Review this P1-associated proposal first. Accept it as conditional documentary R09 preparation, request a narrower fence, or retain HOLD. An eventual BUILD decision must bind exact owner returns/limits, source/candidate identities, author/reviewer, paths/effects, deadlines and the stated exclusions; **accepting this draft alone is not that decision**. R09 stays PARTIAL/proposed until authentic disposition and actual future assignments exist; no other R row is silently answered.

All B1 R2/M1, B7 HOLD+CLARIFY, G3 native-resume+procedural, B9 parked, S3/owner/D4D7/E14+/I1a–5a and merge/PR restrictions remain. No B7/Jev/privacy design is inside this proposal. No implementation or adoption follows today's documentary review.



> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
