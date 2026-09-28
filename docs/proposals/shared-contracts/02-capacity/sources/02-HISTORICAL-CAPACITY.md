# 02 — Advisory-local historical capacity (CAP-A)

DRAFT ONLY · no implementation/adoption grant. B7 code remains HOLD.
All acceptance controls below are PROPOSED, NOT RUN; no current-owner conclusion.

<a id="authority"></a>
## Authority and supersession

Scope is advisory-local ([scope](./private-evidence/kevin-words-reuse-map-20260916/DISPOSITIONS.md:16)).
[A2](./private-evidence/beacon-overnight-v2-20260917/MUSE-DISPOSITIONS-20260917.md:11) selects CAP-A, not CAP-B/C and
their suggested60/300-second freshness limits. The accepted design-lane pattern
is [CAP-A design](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:41),
[acceptance](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/ACCEPTANCE.md:39) and
[named inputs](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/INPUTS-NEEDED.md:7); its independent review is explicitly
document-only ([review scope](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-review/REVIEW.md:12)).
No runtime or semantic implementation PASS is inherited.

The older capacity as-of/allowed-age wording narrows to a caller-selected historical
interval. Live freshness is always UNKNOWN. Later B7 progress does not close its
privacy/profile finding ([B7 hold evidence](./private-evidence/beacon-weekend-v2-20260919/b7-blue-r2/REVIEW.md:17)).

<a id="interface"></a>
## Inputs, outputs and ownership

One read-only module interface:
`reconcile(supplied_selection, comparison_policy) -> advisory_report`.
No source discovery, live clock/process census or issuer is introduced.
Canonical claim/observation owners retain their authority; this module compares
supplied records only. Deletion loses the advisory view, not any lease.

| Input | Required exact binding / missing behavior |
|---|---|
| Selection/mode | Explicit local root and finite reference list; synthetic versus supplied-artifact mode. Missing selection is unknown/input-error before evidence reads. No synthetic descriptor fills missing real authority. |
| Resource | One explicitly identified node-a compiler resource, approved host descriptor and explicit aliases (including deliberately approved empty set), relevant tree/head/cache bindings. Missing/unapproved alias means unknown join. |
| Historical policy | Requested interval; supported domains/epochs/units/precision and effective owner-order semantics; coverage; acquisition/heartbeat/release/expiry/exclusivity meanings. Missing relation is unknown; no current-as-of or TTL. |
| Claim | Resource/host, seat AND session, issuer, claim ID AND generation, work/brief; each event separately witnessed with effective order/time. No invented lease. |
| Presence/absence | Presence binds host+boot+PID+start and independent resource/claim/generation/session association. Absence has scoped complete-census evidence and no invented process tuple. Silence/idle/empty input proves nothing. |
| Artifact/custody | Exact confined path+full digest+format/version+selector and source-bound value; independent Muse-approved producer/custodian scope. Correct hash/self-approval is not custody. |
| Bounds/formats | Explicit finite positive artifact/byte/total/record/reference/text/depth limits and numeric domains. Missing policy blocks CAP evidence reads; no fixture-number defaults. |
| Optional legacy companions | Exact dispatch and checkpoint+packet/head context. Preserve original output/refusal, no fallback count or changed schema. Omitted is not supplied, never empty success. |

Source: [design input table](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:22),
[missing actual identities](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:58).
Output contains requested/supported scope, `agreed / conflicting / unknown`,
contributing and adverse safe references, named gaps/blocked indices/input errors,
and **live_freshness=UNKNOWN**. Worker/checkpoint results are separate companion
results/refusals. No global PASS, free/ready count, current-owner, exclusive-custody,
dispatch, permission or publication result
([report contract](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:82)).

<a id="semantics"></a>
## Error, uncertainty, retry and ordering

1. Shape, bounds, source bytes/selectors, custody, identity and relation checks
   precede admitting evidence. Unknown tags/duplicate keys/nonfinite values/type
   coercion fail closed. Counts and bytes charge selected occurrences before
   deduplication; refused/failed reads do not refund consumed bytes. No truncation
   into complete coverage. Existing reader ceiling is an upper ceiling, not a
   selected CAP allowance.
2. Join only approved aliases and exact host+boot+PID+start, with separately
   witnessed claim/generation/session association. PID or seat title alone never
   joins. Missing global prerequisites may leave all comparisons unknown.
3. Effective owner order is not B1 delivery-observer order. Comparable strict-before
   requires separated uncertainty intervals; equal instants are not before and
   overlapping uncertainty is unordered. Expiry equality semantics come from the
   issuer, never a guessed TTL.
4. Proven contradiction yields conflicting with all adverse refs AND gaps.
   Otherwise sufficient compatible evidence plus complete required coverage may
   agree; otherwise unknown. An unrelated invalid input cannot erase a proved
   conflict, but removing a contradiction prerequisite may make it unknown.
5. Presence after proved release/expiry, same-generation heartbeat after release,
   or overlapping evidenced exclusive claims conflicts. Before-release heartbeat
   filed later cannot reacquire; new generation needs acquisition. Complete
   scoped absence supports only its observed scope/point, not a broader interval.
6. Retain distinct path+digest sources even when payloads match. Immutable identity
   with divergent bytes conflicts. Retry/reorder identical supplied bytes cannot
   change comparison or erase history; changed inputs are separately pinned.
   Later agreement does not erase earlier supplied historical conflict.

Basis: [typed/bounded refusal](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:60),
[comparison rules](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:72),
[historical accounting correction; not acceptance](./private-evidence/beacon-weekend-v2-20260919/b7-author-r2/candidate/PROFILE.md:184).

<a id="privacy"></a>
## Privacy, trust and explicit non-effects

Approved producers sanitize operational artifacts **before supply/persistence**.
No argv/env, raw terminal/model text, tokens/credentials or personal contact data.
Opaque IDs and hashes are not sanitization. Invalid evidence gets fixed safe
reason/input index, not raw path/body/OS exception. Independent admissible safe
evidence remains visible. Actual producer/privacy correctness is UNVERIFIED.
Source: [privacy obligation](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/DESIGN.md:70).

Do not reclassify recorded workers as compilers: inspected
[dispatch summary](./private-evidence/tools/dispatch_check.py:131) counts recorded
nonterminal lane reservations. Preserve legacy malformed/overlap/wrong-head/prior
unresolved refusals. No live lease/census/kill/release/renewal/reassignment, network,
subprocess, watcher, shared-schema write or publication. B2-A remains obligations
and reviews only; compiler comparison is a separate view, not a new facts kind.
B3 membership and K7 acknowledgements never establish compiler order/release.
Missing-checkpoint refusal and MUSE-ONLY / UNKNOWN publication remain unchanged.

<a id="acceptance"></a>
## Acceptance matrix — PROPOSED, NOT RUN

All rows require literal statuses/scopes/witness sets, safe errors, unchanged
sources and zero effects. The ten original full IDs are individually bound in
[ACCEPTANCE-MAP.md](ACCEPTANCE-MAP.md); inherited14 CAPA design rows remain proposed,
not an executed39-case or86-test claim.

| Original prefix | Positive control | Contrary control / scope narrowing |
|---|---|---|
| CAP.01 | Matched historical held claim/process with established complete scope may agree; worker result unchanged. | Vary worker counts alone: never changes compiler result or creates a free slot; current-fresh interpretation UNKNOWN. |
| CAP.02 | Supported observation covers selected interval. | Missing host/coverage/interval or outside/incomparable observation unknown. Current-age staleness and freshness-TTL variants deferred, no TTL default. |
| CAP.03 | Complete release/expiry order and associated later presence conflict. | Missing relation/issuer expiry equality semantics unknown; no automatic expiration or action. |
| CAP.04 | Exact host+boot+PID+start association holds. | Reuse PID with changed host/boot/start unknown association; retain both tuples. |
| CAP.05 | Separately bound new generation/session has its own acquisition. | Old claim cannot cover replacement; supported overlapping exclusive claims conflict without winner. |
| CAP.06 | Effective before-release heartbeat filed late plus complete scoped absence may agree there. | Effective after-release same-generation heartbeat conflicts; only delivery-observer order or missing relation unknown. |
| CAP.07 | Two admitted exclusive claims overlap: conflict retains both. | Add invalid same-lane dispatch: keep worker refusal/no count, not a collapsed compiler result; remove exclusivity proof => unknown. |
| CAP.08 | Released claim + complete scoped historical absence agrees only at supported scope. | Partial/empty/idle/unknown or point census offered for whole interval unknown; no current-free label. |
| CAP.09 | Valid pinned/confined bounded input with independently approved custody can be admitted. | Drift/escape/nonregular/oversize/missing custody => blocked unknown/input-error; not absence; unrelated proved conflicts retained. |
| CAP.10 | Safe approved synthetic operational data yields permutation-stable comparisons. | Forbidden fields or synthetic sensitive markers refuse without echo; inert text has zero effects; filing time cannot erase conflict. |

[accepted design mapping](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/ACCEPTANCE.md:18) controls historical
narrowing. CAPA.09 actual race/midread, CAPA.12 full JSON text/depth equality and
CAPA.14 wrong-head/unresolved-prior remain UNVERIFIED as recorded by
[residual review limits](./private-evidence/beacon-weekend-v2-20260919/b7-blue-r2/REVIEW.md:77). No tests run here.

<a id="missing"></a>
## Missing inputs, owners and supersession map

Retain [R/C/T/O/B/L owners](./private-evidence/beacon-overnight-v2-20260917/a2-cap-a-design/INPUTS-NEEDED.md:8): Muse approves resource aliases,
custody and comparison semantics; source/deployment owners supply actual issuer,
observer, host/boot/session/process association and sanitized scope-complete
artifacts; caller explicitly chooses the historical interval; Muse/delegate names
finite bounds/formats; conductor supplies legacy pins, later author/reviewer and
exact synthetic build grant. No producers or auth mechanism are invented.

Supersession: original live/as-of capacity → CAP-A historical-only; CAP-B/C
unselected; design-review PASS is documentary only; candidate privacy remains HOLD
pending proposal05 decision plus a separate implementation grant. No fixture caps,
clock defaults or fake custody become real policy. Dependency sought: remembering
historical ownership disagreement; no claim of live availability or measured savings.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
