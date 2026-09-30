# 03 — Artifact-only delivery evidence join

DRAFT ONLY · no dispatch/adoption grant. Every acceptance row is PROPOSED, NOT RUN.
No send, acknowledgement, live observation or receiver identity is created here.

<a id="authority"></a>
## Authority and supersession

Artifact-only scope: [scope record](/home/kevin-lee/ai-workspace/codex/reports/kevin-words-reuse-map-20260916/DISPOSITIONS.md:16).
Later [A2 selections](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-v2-20260917/MUSE-DISPOSITIONS-20260917.md:11) selects
[observer-local ordering](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B1-EVENT-ORDER.md:9) and
[four independent indicators](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/K7-DELIVERY-STAGES.md:15).
The R2 compact stage family and stronger late-ack wording are no longer controlling.
B1-B causal partial order and K7-A compact display are unselected. Receipt evidence
still does not mean consent, authority or task PASS. Original/R2 acknowledgement
custody choices remain unresolved; neither ordering nor presentation selection
names a trusted acknowledger ([custody source](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/DELIVERY-R2.md:79)).

<a id="interface"></a>
## Inputs, outputs and ownership

One run-owned, read-only local module interface consumes explicitly supplied
directive, attempt/event, stage-observation and custody/profile references and
returns a delivery evidence view. No native hook or new shared event schema.
Directive and source-observer owners retain original evidence; the conductor owns
the selected local join artifact, not a receiver's identity. Deleting the module
loses the joined view only, not recorded dispatch or communication capability.

| Input | Proposed required binding |
|---|---|
| Directive | Stable ID and exact permitted artifact path/full SHA256. No altered wording under the same immutable identity. |
| Attempt | Unique attempt ID/ordinal, exact directive digest, exact target host/pane/session and sender provenance. Attempt metadata alone does not prove send. |
| Stage evidence | Posted, sent, transport-observed and acknowledged each has its own source-format-backed witness, exact subject/attempt/target scope as applicable, and approved provenance. Missing a stage witness leaves that indicator unknown. |
| Acknowledgement | Exact directive bytes, attempt, target host/pane/session, claimed receiver provenance AND independently established custody. Text forwarded by transport or seat title does not prove who acknowledged. |
| Ordered events | Observer identity/epoch, stream ID, sequence, attempt/hash/target and pinned event witness for send, withdrawal request, ack observation and withdrawal-ack observation. Sequence must occur in the pinned observer record; never assigned by this consumer. |
| Profile | Approved observer/acknowledger custody, source formats/selectors, coverage inventory and finite artifact/event/reference/text/byte bounds. Missing values stay named unknown/refused inputs, no numbers/identities guessed. |

Output shows **posted, sent, transport-observed, acknowledged**, each
`supported / unknown / contradicted` with its own permitted references and gaps.
There is **no implied stage progression**: even a valid ack does not manufacture a
missing send/post/transport witness. Withdrawal request, observed withdrawal
acknowledgement, observed-after-withdrawal conflict and UNORDERED/coverage annotations
remain separate. No overall approved, ready, consent, dispatch or publication result.
Basis: [original exact join](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/DELIVERY-R2.md:37),
[selected output contract](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/K7-DELIVERY-STAGES.md:15).
The old universal “nonempty send witness for any attempt” is narrowed: send support
requires it; later independent observed evidence may exist while sent remains unknown.

<a id="semantics"></a>
## Error, uncertainty, retry and ordering

B1-A compares only one witnessed observer-local stream within an attempt. Ack
observed after withdrawal request means **observed-after-withdrawal conflict**,
not remote generation-after-withdrawal or causality. An old receiver ack may arrive
late; it does not revive a withdrawn attempt or grant authority. Distinct
observer/epoch without a witnessed relation stays UNORDERED; no B1-B graph is
silently adopted. Equal sequence position/different bytes is CONFLICT; exact repeated
event identity+bytes may coalesce with all provenance retained. Gaps remain visible.
Input/file arrival order or wall-clock timestamps cannot fill missing positions.
Source: [order/conflict semantics](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B1-EVENT-ORDER.md:11).

Malformed attempt identities, duplicate attempt ID/ordinal or non-increasing declared
attempt ordinal refuse that input. A new transmission retry needs a new attempt
ID/ordinal, exact current target and its own evidence; preserve the old attempt.
Reading the same immutable supplied selection again produces the same view.
Event coalescing is not permission to collapse different attempts or duplicate
attempt definitions. Replacement session cannot inherit an old target's ack.

Wrong directive hash, changed artifact, missing identity/custody or mismatched
target never supports this attempt. Safe classification/blocked index is retained;
no derived stage support from invalid records. Unrelated valid evidence remains
scoped, not overwritten by a synthetic overall success. A proposed CLI may retain
REFUSED/empty normal stdout for structurally invalid joined input; diagnostic
output uses closed safe labels, and exact envelope/exit specification needs later
admission. UNKNOWN missing stage evidence is not structural success or failure.
Legacy dispatch exits/refusals are unchanged; terminal/start observation alone
does not establish a send or acknowledgement
([dispatch notices](/home/kevin-lee/ai-workspace/codex/conductor-tools/dispatch_check.py:56)).

<a id="privacy"></a>
## Privacy, trust and explicit non-effects

Only approved sanitized directive excerpts/operational IDs and source references
are supplied. No raw conversation, terminal/model output or credentials copied;
untrusted strings, paths and exceptions are never echoed on failure. Hash equality
proves neither receiver identity nor reading/acceptance. Custody model (receiver-
authored evidence versus conductor-observed acknowledgement) must be labelled and
approved, not silently chosen. Native structured acknowledgements remain a separate
shared-interface decision ([native-hook hold](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/DELIVERY-R2.md:87)).

No prompt, send, cancel, withdrawal request, acknowledgement write, dialog input,
network probe, watcher, session operation, authorization, Flow use or publication.
Observer sequence evidence is not a capacity lease/release. Inputs immutable;
retries are evidence joins only, not retry scheduling.

<a id="acceptance"></a>
## Acceptance matrix — PROPOSED, NOT RUN

Every row uses independent literal indicators/annotations, preserves raw permitted
references and checks zero effects. Original D1–D9 remain individually mapped in
[ACCEPTANCE-MAP.md](ACCEPTANCE-MAP.md), all runtime UNVERIFIED.

| ID | Positive control | Contrary control / selected narrowing |
|---|---|---|
| D1 | Exact ack binding with approved custody supports acknowledged only. | Wrong target/provenance/bytes cannot support it; remove earlier stage witnesses and those indicators stay unknown. |
| D2 | Own send witness supports sent with acknowledged unknown when no ack exists. | Start/terminal/post/transport alone never implies sent or acknowledged; R2 SENT_UNACKNOWLEDGED shorthand is narrowed accordingly. |
| D3 | Exact immutable directive/witness digests bind. | Changed/wrong/missing artifact refuses evidence; no unsafe normal-output echo. |
| D4 | Each session has separately bound attempt and ack. | Replaced pane/session cannot inherit old ack; preserve it only as historical evidence for its original target. |
| D5 | New retry ID/ordinal preserves old attempt; exact repeated observation may coalesce provenance. | Duplicate attempt ID/ordinal/non-increasing declared ordinal refuses; event duplicate with conflicting bytes remains conflict, not retry success. |
| D6 | Withdrawal request and exact matching withdrawal-ack each evidenced separately. | Request alone does not imply acknowledgement, remote cancellation or ceased dispatch; coverage gap remains. |
| D7 | Same observer/epoch/stream witnesses request then ack observation: observed-after-withdrawal conflict. | Delayed old remote ack does not prove remote causality; different epochs/unwitnessed order UNORDERED; file permutation unchanged. |
| D8 | Complete sender/receiver/observer provenance and independently approved scope permit relevant indicator. | Missing host/pane/session/witness or custody stays refused/unknown; never fill from seat title/actor string. |
| D9 | Permitted command-like text remains inert; before/after source bytes identical. | No process/network/UI effect or grant interpretation; private/error text never leaks. |

Additional discriminator within D1/D2: support each of the four indicators alone
with its own valid witness, keep all other indicators unknown. A wrong shortcut
“ack implies every prior stage” must fail. Within D7, a wrong shortcut “arrival
order implies receiver chronology” must fail. These are proposed controls, not
additional executed scenarios. Sources: [original case family](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/revisions/DELIVERY-R2.md:69),
[ordering discriminators](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/B1-EVENT-ORDER.md:23),
[stage independence](/home/kevin-lee/ai-workspace/codex/reports/beacon-overnight-revision-20260917/options/K7-DELIVERY-STAGES.md:15).

<a id="missing"></a>
## Missing inputs, owners and supersession map

Muse must approve source/acknowledger/observer custody and supported format meanings;
actual source/deployment owners supply descriptors and independently inspectable
provenance, event stream/epoch and coverage. Conductor supplies explicit selected
artifacts, future bounded author/test fence and reviewer; delegate supplies finite
limits. Native hooks/shared schema need separate owners; Flow review before any
Flow consumption. This proposal invents no receiver identity/channel.

Supersession: original/R2 D1–D9 → same evidence obligations with B1-A/K7-B
narrowings; D2 no unsupported send inference; D7 local observation only; compact
stage list replaced, not merged into a total progression. Accepted B1-A is an
option label, not the unrelated B1 product/build lane.
Dependency sought: remembering which relay steps are actually evidenced, not
performing those steps or measuring operational savings.
