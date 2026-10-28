# I1 and PBDEP work in progress — 2026-10-27

This branch preserves source preparation and qualification work for review. It is
not a release, a passing integrated product gate, or a ready-to-merge candidate.
The base is sanitized main `4e05784f8dd3fa6017cf42df5a47b612265c3f07`.
No pre-cleanup ancestry or private report corpus is imported.

## Included work

| Area | Source | Current evidence and limits |
|---|---|---|
| I1 pilot contract | `crates/beacond/src/pilot_{admission,authority,store}.rs`, dispatch/project/lib joins, pilot tests and disposable SQL fixtures | Sanitized source preparation reviewed. Not a new live migration or product acceptance. |
| PBDEP raw/canonical identity | `scripts/qualification/pbdep/contract.py`, `policy.py`, `whole_program.py` | Preserve compiler spellings separately from verified canonical identities. Source review is not a new compiler run. |
| Observation and custody | `runtime.py`, `supervise.py` | Bounded capture, observation and refusal machinery; real-provider and whole-program qualification remain separate. |
| Discovery | `entry.py`, `discovery.py`, `qualify_whole_program.py` | Reviewed discovery source preparation; no authorization or host qualification follows from this branch. |
| Controls | `tests/checks.py`, `controls.json`, `fixture-files.json`, `replay-map.json` | Current held control definitions, including aggregate-family cases. Known defect below; the complete suite has not been credited as passing. |
| Contracts | `i1-pilot-contract.md`, `pbdep-interface.md`, `pbdep-discovery.md` | Design and interface records, with their own qualifications and limits. |

The latest source snapshot preserves 75 global control IDs and 65 selectors.
The aggregate test method contains a 972-row positive, 14 original negatives and
16 cardinality definitions: 31 trial shapes, not 31 passing tests.

## Known blocking fixture defect

`test_b05_q_aggregate02_full_synthetic_chain_and_isolated_join_guards` is held.
Its generic oracle filename selects a smaller retention cap before the later
control-sized read. Merely changing the oracle/results names is insufficient:
producer `.source` names reach a JSON-only fallback filename grammar before the
family guard, and `row-input.bin` reaches it later on row-consuming paths.

These additional observations are source-derived conditional refusals, not newly
executed failing cases. The latest repair pass stopped before editing. Reader
caps, expected outcomes and assertions have not been weakened to conceal them.
The next proposed work is a method-local fixture artifact-class and join repair,
with independent review before any separately admitted synthetic qualification.

## Qualification evidence kept separate

A separate, host-bound A2 consumer exercise completed one preflight and one
eight-case synthetic run. All eight expected outcomes matched, two 736-record
native inventories passed, and six newly reserved capture handles were observed
closed. Independent source, filled-packet and retained-result reviews found no
new findings at those narrow scopes.

That exercise is not the aggregate control suite, authentic product provenance,
host descendant containment or a complete I1/product test. Its positive case
classifies a supplied synthetic witness; it does not represent newly executing
the underlying product tests. Original-domain custody remained unproven.

The host-specific `probe_a2_receipt.py`, its absolute private evidence bindings,
raw runtime captures, launch packets, private reports and historical recovery
repositories are deliberately excluded from this public snapshot. No included
module references that omitted probe by name. Reconstructing a portable probe
would require a separately reviewed change; this branch does not claim one.

## Publication verification versus product readiness

Publication checks cover source-byte conservation, Python AST parsing without
imports, diff whitespace, a bounded private-reference/credential pattern screen
(including six compressed literal fixtures), and independent publication review.
They do not run candidate code or transfer historical test results to this branch.
The full product gate and current aggregate controls are **UNRUN** for this snapshot.

Whole-program binding, integrated qualification, real-subject execution, live
migrations, deployment and landing remain pending. Dependency remediation is also
pending: prior triage found three development packages associated with six high
advisories, without an identified shipped exploit path; that is a dated triage,
not a fresh dependency scan. Operator CI configuration remains separately needed.
No dependency, workflow, runner, credential or service setting changes are included.

Keep this PR in draft until the blocking source work and required verification
are completed and independently reviewed. Main remains the accepted product state.
