# Overnight documentary integration — 2026-09-29

**DRAFT ONLY.** This note makes the existing discovery packet reviewable on the landed base; it implements no API, policy or product behavior. M6 assumptions remain **PROVISIONAL**, Kevin-reversible, and unusable for implementation until Kevin ratifies or replaces them. Nothing here grants landing permission.

## Base and preserved inputs

This integration starts at Beacon main `a48bae4cda2502c07a17ab747aa7e429b3ec5843`, observed on 2026-09-29 at about 22:05 EDT. PRs #162–166 were merged at that observation. Their landed documentary packets and CI repair are not new grants for held work. No future remote freshness is asserted.

The twelve prior discovery files are byte-identical to the local candidate `26a4e7ce7b5e7bbd4c9abf6e80678630cabc8ad9`. They were replayed onto the new base without changing their historical parent/source labels or manifests:

| Original commit | Replay commit | Preserved increment |
|---|---|---|
| `75969d8613eb377a2b9a9499dffee75a9dd54fdc` | `3248b600b960850398ddc32ac70db6609c253f12` | AC22 Assignment recovery and AC27 Attention/notification witness boundaries |
| `e992e8ee9c2fb56e19a683164f46930064038d46` | `28d8c2ccab602cf038b39e36320baa79265c3d36` | AC40 current Attention versus acknowledged history |
| `26a4e7ce7b5e7bbd4c9abf6e80678630cabc8ad9` | `719beff0dc2a4e17bdbb3e4bd2ff0923596dec12` | Provisional M6 callable/state contract and source compatibility map |

Their original references to `84fc76a...` or `e992e8ee...` describe historical source/authoring context, not the current integration head. The final candidate head is recorded in the accompanying exact-head review packet rather than a self-referential commit label here. Replay identity carries documentary bytes and historical evidence; it does not transfer Muse's approval to a new head.

## Read the packet by evidence type

- Existing accepted recovery and Attention requirements are cited in the [recovery boundary](assignment-recovery-boundary.md), [notification boundary](attention-notification-boundary.md) and [history boundary](attention-history-boundary.md). These discovery drafts catalogue required witnesses and gaps; they are not proof that the requirements are implemented.
- The [M6 memo](m6-provisional-dispositions.md) supplies seven **PROVISIONAL** documentary assumptions. The [callable/state contract](m6-assignment-attention-contract.md) labels dependent assumptions and witnesses individually as **PROVISIONAL M6-A1/A2/A3/A4/C1/C2/C3**. Kevin's replacement requires rebinding dependent conclusions; ratification supplies neither missing witnesses nor permission to bypass another hold.
- The [source compatibility map](m6-source-compatibility.md) separates current Operator-only actor mappings/SQL checks, historical Invocation evidence and measured spend from proposed Resident attribution, grant admission and nested-budget obligations. Source tests were read; they were not run as proof of the proposed contract.

The M6 semantic review originally rejected applying actor-plus-reason denial-record requirements to Invocation refusal without a covered-command decision. The corrected retained draft leaves that applicability unanswered under Q2. A separate mechanical validation PASS did not override the semantic HOLD; the corrected revision received fresh non-author review and validation.

## Local verification and its limits

The replay verification checked all **118 source records across five manifests**, representing **86 distinct root/path locators**. Repo inputs were checked against their pinned full Git refs and current candidate bytes; workspace inputs against retained bytes. All twelve discovery files matched the prior candidate. The original packet's **156 local links/line anchors** resolved before adding this integration note. Final packet link totals and exact hashes are recorded after this note is frozen.

These checks demonstrate source/target integrity, not automatic semantic entailment. They do not exercise product, runtime, native, packaged, live grant or budget behavior. Source files at historical Git refs must remain available for a cold re-run; the retained workspace inputs are also part of the evidence package and are not all repository files. No missing input may be silently substituted.

Verification artifacts live on mickey under `~/ai-workspace/codex/reports/beacon-joint-overnight-20260929/`: `REPLAY-IDENTITY.json`, source/link logs, final freeze and independent review/validation returns. A final review status belongs to its exact head and evidence, never to this note alone.

## Current queue and stopping boundaries

| Item | Status at this integration |
|---|---|
| This documentary candidate | Local draft; non-author review and validation precede a local exact-head packet for Muse. Nothing is pushed or landed by this note. |
| **PROVISIONAL M6-A1/A2/A3/A4/C1/C2/C3** | Documentary use only. Final semantics require Kevin ratification/replacement before implementation. |
| Q1 recovery eligibility/lineage | Unanswered; no algorithm or receiving-Agent decision. |
| Q2 actor/command/grant/denial scope | Unanswered, including whether Invocation refusal is covered by **PROVISIONAL M6-A3**. No new variants, schema, grant resolver or denial owner. |
| Q3 budget mechanics | Unanswered beneath **PROVISIONAL M6-C2/C3**; no reservation, charge/refund, concurrency or recording order. Existing accepted parent bounds remain. |
| Q4 Attention lifecycle/eligibility | Unanswered; a denial is not automatically an Attention Item. |
| Q01–Q06 | Authentic owner returns still required. Proposal/disposition paperwork is not custody evidence. |
| B1/M1, B7, G3/capacity, P1-Muse-first | Existing holds remain. Merged shared-contract proposals do not release them. |

Beacon verification remains local-only under the intentional $0 hosted-Actions policy. Every landing remains Muse's same-flow exact-head lane, with existing owner gates intact. Work needing Kevin's explicit tap is parked. No provisional product implementation, authority/permission/Tier change, main push, merge, close, retarget, force-push, hosted run or deletion is authorized.
