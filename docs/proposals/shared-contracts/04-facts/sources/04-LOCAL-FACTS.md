# 04 — Local facts display relative to immutable baseline membership

DRAFT ONLY · local display, not another authority store or time-based “latest” view.
All acceptance controls are PROPOSED, NOT RUN; no implementation/adoption grant.

<a id="authority"></a>
## Authority and supersession

Local-display scope is selected ([scope](/home/kevin-lee/ai-workspace/codex/reports/kevin-words-reuse-map-20260916/DISPOSITIONS.md:16)).
[A2](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-v2-20260917/MUSE-DISPOSITIONS-20260917.md:11) selects
[closed obligation/review payloads](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B2-FACT-PAYLOAD.md:9) and
[immutable baseline membership](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B3-CHECKPOINT-BOUNDARY.md:9).
B2-B compiler facts and B3-B time-cutoff policy are unselected. R2 identity-only
observation fields are insufficient to compare meaning and are completed by B2-A;
R2 F02's checkpoint-retrospective wording is narrowed by B3-A. No shared checkpoint
timestamp or new global fact schema is introduced.

<a id="interface"></a>
## Inputs, outputs and ownership

One read-only module interface consumes a pinned checkpoint plus its packet/head
context, separate pinned local baseline-selection artifact, explicit observation
references and supported local profile. It returns a derived facts view; existing
packet/checkpoint/registry records remain the truth of their recorded claims.
Removing the view loses only local comparison/presentation.

| Input | Required meaning |
|---|---|
| Baseline selection | Exact checkpoint digest and explicit set of already-included immutable observation identities/hashes, plus declared source coverage. Missing membership/coverage means UNKNOWN, not an empty baseline. |
| Obligation payload | Literal ID, scope, exact assertion text, status PASS/FAIL/SKIP, reason, witness reference, typed subject from validated packet context. No invented fields/status from unreadability. |
| Review payload | Reviewer, scope, revision, disposition PASS/FAIL/HOLD/PARTIAL/WITHDRAWN, explicit supersedes link and report reference, exact typed subject. |
| Observation envelope | Source format/version, pinned raw artifact and selectors, source identity/provenance, version identity, observed_at and recorded_at with precision or unknown, explicit supported relationship and coverage slot. |
| Format/custody/bounds | Muse-approved finite formats/selectors, disclosure and custody descriptors, comparison domains and artifact/byte/record/reference/text/depth limits. Omitted inputs stay named unknown/errors. |

The closed payload union is **obligations and reviews only**; no `value:any`,
compiler, lease, gate or publication fact kind. The source review/obligation
enums are visible in [review validation](/home/kevin-lee/ai-workspace/codex/conductor-tools/packet.py:222)
and [obligation validation](/home/kevin-lee/ai-workspace/codex/conductor-tools/packet.py:323);
static inspection does not prove the proposed adapter.

Output: bound checkpoint/baseline reference, newly-seen rows, explicit correction/
withdrawal relationships, unresolved conflicts and coverage gaps, all safe source
references and timestamps. View generation time, if displayed, is `rendered_at`,
not completion or source-observed time; do not alter legacy current_work output.
There is no readiness, authority, dispatch or publication result. Existing packet
publication text may only be retained as separately attributed original display
context, not a newly derived fact/decision.

<a id="semantics"></a>
## Error, uncertainty, retry and ordering

A logical key includes kind, typed subject and source-defined entity identity:
obligation ID+scope or reviewer+scope. Version identity is separate. Compare typed
payload fields exactly; no trimming, paraphrase, fuzzy matching, implicit type
coercion or model interpretation. Equal substantive payloads with different witness
references retain ALL provenance; differing witness refs alone do not prove
contradiction. Different bytes for one immutable version remain CONFLICT.
Divergent comparable statuses/assertions without validated revision relationship
remain CONFLICT; missing fields, unsupported kinds/semantics or unestablished
custody remain UNKNOWN, not a fabricated SKIP.
Basis: [typed equality](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B2-FACT-PAYLOAD.md:14),
[conflict/unknown](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B2-FACT-PAYLOAD.md:16).

A supplied observation outside the explicit baseline set is **newly seen relative
to this baseline**, not temporally newer. A missing baseline is not a claim that
every record is new. Keep observed/recorded values and unknown precision; Monday
observation and Wednesday filing may be distinguished only when source evidence
supports it. “Retrospective relative to checkpoint time” remains UNKNOWN because
B3-A has no cutoff. Neither file mtime, arrival order nor rendered_at supplies one.
[selected narrower time meaning](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B3-CHECKPOINT-BOUNDARY.md:11).

Input permutation/rechecking identical immutable selection leaves the same facts/
references. Changed selection is a separately pinned baseline revision, never
mutation of an old checkpoint. Valid source-owned supersession/withdrawal changes
displayed relationships while preserving predecessors; timestamp alone never
selects a winner. Different head/scope cannot close old obligations. Show existing
FAIL/SKIP even if a set difference would otherwise omit them. Only already evidenced
authoritative packet transitions can be displayed as transitions; this module
cannot make one ([legacy retention](/home/kevin-lee/ai-workspace/codex/conductor-tools/packet.py:384)).

Invalid/mismatched baseline binding or unsafe/unreadable artifact refuses that
input and leaves an explicit gap; it cannot become “no facts” or success. Retain
independently valid evidence where the approved result envelope permits. Changed
bytes for a referenced immutable version are adverse, not a silent retry update.
Exact envelope/exit behavior for a future local adapter needs its own frozen
profile, never a change to existing reader refusals.

<a id="privacy"></a>
## Privacy, trust and explicit non-effects

Only approved sanitized operational observation fields/references may enter the
view. Custody and permitted output labels are prerequisites; a hash, arbitrary
actor string or “raw reference visible” rule does not permit private text.
Unsafe input uses a safe index and closed diagnostic; no raw path/exception echo.
An unsupported fact can be represented by safe format/index and missing-input
reason without copying disallowed payload. No semantic private-data classifier
or real producer authenticity is claimed.

No mutation/closure/watch/collector/lease/gate/execution/dispatch/publication method,
no source/time/host lookup and no shared handoff/checkpoint extension. Compiler
comparison remains solely proposal02; delivery indicators cannot create facts
outside the closed union. Source quotations preserve original bytes only where
disclosure permits them. Sources: [local non-effects](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/FACTS-R2.md:29),
[no open-ended union](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B2-FACT-PAYLOAD.md:28).

<a id="acceptance"></a>
## Acceptance matrix — PROPOSED, NOT RUN

Each row checks literal typed payloads/relationships, safe provenance, unchanged
source hashes and zero effects. See [ACCEPTANCE-MAP.md](ACCEPTANCE-MAP.md) for all
original F01–F10 individually, including explicit narrowing.

| ID | Positive control | Contrary control / selected narrowing |
|---|---|---|
| F01 | Permuted valid obligation/review observations yield identical rows/witness sets and retain timestamps. | Arrival order cannot select winner; immutable-version byte disagreement remains conflict. |
| F02 | Old observed time/later recorded time remains distinct; absent-baseline member is newly seen. | No checkpoint cutoff invented; “retrospective relative to checkpoint time” UNKNOWN; unknown precision stays unknown. |
| F03 | Explicit supplied source/coverage reports known facts only. | Missing/unavailable expected source means UNKNOWN coverage, never absence/completion/free capacity. |
| F04 | Exact same-subject validated withdrawal retains PASS predecessor and withdrawal relationship. | Later unlinked contradiction is conflict, never timestamp supersession. |
| F05 | New head B is a different typed subject, additions relative to baseline. | B PASS cannot close A; equal-length Git/artifact digest types never join. |
| F06 | Prior FAIL/SKIP remain under same ID/scope/assertion. | Omission/relabel/narrow PASS cannot clear broader obligation; no automatic closure by display. |
| F07 | Comparable closed-kind equal payloads agree as recorded statements with all refs. | Divergent same-key status/assertion remains CONFLICT even if newer; arbitrary authority/compiler/publication claims outside union unsupported/UNKNOWN, not coerced. |
| F08 | Valid review PASS displayed as review only. | Gate/review evidence cannot infer publication; gate/publication remain unsupported kinds; any original packet literal stays separately attributed context. |
| F09 | Valid confined stable pinned artifact yields only its supported payload. | Drift/escape/symlink/bounds refuse the input, not a successful empty result; raw files unchanged. |
| F10 | Sole registry source/correction and explicit omit reason remain reachable. | Missing/stale/disagreeing disposition refuses; no duplicate registry or timestamp priority. |

Original case family: [F01–F10](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/FACTS-R2.md:48).
B2-A narrows generic fact/authority rows and F08; B3-A narrows F02.
Wrong-shortcut controls must defeat timestamp-as-authority, empty-baseline-on-error,
arbitrary-kind acceptance and dropping carried FAIL/SKIP. All are unexecuted.

<a id="missing"></a>
## Missing inputs, owners and supersession map

Muse approves closed payload/relationship semantics and the exact local profile,
baseline membership custody/coverage and source adapters. Source/run owners supply
pinned observations and exact source field semantics; admission delegate supplies
finite bounds, conductor supplies legacy inputs/future fence and different reviewer.
No actual source host, producer, schema version, coverage set or limit is invented.

Supersession: original/R2 local display retained; B2-A adds exact closed payload
meaning and excludes B2-B; B3-A selects set membership and excludes B3-B; source
observed/recorded timestamps remain evidence, not time authority. Generic source
chronology claims not supported by selected membership stay UNVERIFIED, not deleted.
Dependency sought: reconciling observations against a frozen report without model
memory; no claim of a complete inventory or measured savings.
