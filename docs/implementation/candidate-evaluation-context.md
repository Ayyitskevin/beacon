# Historical context for a proposed Outcome-fidelity Evaluation

`CandidateEvaluationService::context(manifest_digest)` is a Workspace/account-bound read.
It authenticates first and owns one deferred read transaction. It returns the exact existing
`StoredOutcomeManifest`, `VerifiedArtifactRecord`, and `MissionView` owning values, plus the
historically admitted non-author Resident/configuration pairs. Empty means the verified Mission
admitted no other Resident. Absent and foreign selectors both return `None`; malformed selectors,
read-limit refusals and broken owner proof are distinct errors. Old Outcome interfaces retain
their existing scoping behavior.

The Outcome helper reuses the complete startup proof, including completion-owner reconciliation,
Outcome Document kind, Mission and accountable-author binding. It does not substitute the lighter
manifest byte reader for that proof. Artifact and Mission values retain their owners' Event proof.
Projected identifiers used by size preflight are only lookup keys; they are subsequently verified
against the owning records before any result is returned. No account lock is nested and no separate
store connection is opened by this service.

## Current recorded evaluator evidence

`current_evaluator_evidence()` returns one `CurrentEvaluatorEvidence` for each historically
admitted non-author, in the same order. It holds that admitted `Evaluator`, the exact verified
current `ResidentAgentView`, latest `ResidentConfiguration`, and, when configured, a
`CurrentActivationEvidence` containing the configured and latest `LocalModelActivation` values.
A later configuration cannot add another Resident or the author to this historical set.

These are recorded facts in the same authenticated transaction. Lifecycle proof reuses the full
ordered transition/predecessor/time chain, exact Event match and current projection reconciliation.
Configuration proof reuses every ordered decision's existing owner verification and predecessor
chain. Activation proof reuses the complete per-decision proof from startup across the configured
resource's history. Nonpositive sequence keys refuse before typed history enumeration. The global
startup passes keep their existing order, key checks, totals and fault classifications; no whole-store
pass or provider inspection is invoked by this read.

The activation proof's resource key is `(Workspace, endpoint, model_name)`. The stored decoder
accepts only Ollama; provider is a closed constant, not another proof-map component. Model and
inspection digests remain exact record facts and do not split a resource history. Scope enumeration
includes every row for that three-field key, so an invalid provider row cannot disappear behind a
provider filter. The existing configuration verifier's historical activation-row dependencies
retain their original proof behavior; the returned current activation resource receives its own
complete chain proof before the public context returns.

`gaps()` preserves every applicable factual mismatch, in this fixed order:

| Gap | Recorded fact |
|---|---|
| `ResidentNotActive` | Current proved lifecycle is not `Active`. |
| `ConfigurationCleared` | Latest configuration state is `Cleared`. |
| `ConfigurationChanged` | Latest configuration ID differs from the admitted ID, including a later cleared decision. |
| `MissingArtifactEvaluate` | Current configuration has no `ArtifactEvaluate`, including no binding after a clear. |
| `ActivationSuperseded` | Configured activation ID differs from the latest decision for its resource key. |
| `ActivationInactive` | The latest activation state is `Inactive`. |

A cleared configuration carries no activation evidence and no activation gaps. A deactivation can
produce both activation gaps. Empty gaps means only that these enumerated recorded facts match;
it establishes no live availability or authority. The existing `ResidentConfiguration` value
includes `eligible_for_team` and its owning Team-specific reasons. They are preserved as owner
fields and never interpreted as Evaluation readiness. Missing or broken Resident, configuration
or activation owner data is a typed owner refusal, never a normal gap or empty evidence list.

## What this read does not establish

`current_eligibility()` is explicitly `NotMeasured`: historical admission does not establish current
Resident lifecycle, configuration, capability, activation or request authority. The added current
recorded facts still do not establish live resources, request eligibility or authority. Reconfiguring an
author preserves the historical pair. Forming a later Team cannot repair an old one-member Mission.
The author exclusion is keyed on Resident identity in the implementation. Valid public-owner
history still binds that author to its original admitted configuration, so the reconfiguration
case proves historical preservation, not discrimination against a configuration-keyed filter in
this narrowed tracer. That latter challenge belongs to evaluator selection and request eligibility, which this read does not implement.

`source_packet()` is explicitly `NotCollected`: the exact candidate and manifest are not the full
frozen owner-verified evidence packet needed to apply a fidelity rubric. This context is never
permission or request readiness. It starts no task or provider request, creates no Invocation,
Evaluation, Run, Receipt or context record, and offers no native command or desktop screen.

The four criteria in `crates/beacond/resources/evaluation/outcome-fidelity-v1.txt` are an inert,
maintainer-authored **draft**. Their stable IDs are `completion_fidelity`, `evidence_fidelity`,
`honesty_states`, and `limitations_and_authority`. The returned SHA-256 covers the exact UTF-8
file bytes using `beacon.raw.v1`. The rubric is not an Agent-authored Artifact, accepted product
Policy, a claim it was applied, or an authorization to spend resources.

## Direct read bounds and their limits

Preflight measures the following directly selected values with SQL `length()`/aggregate queries
before this context's owning reads transfer them into Rust-owned values:

| Direct selection | Read-owned limit |
|---|---|
| Manifest canonical bytes | Existing `MAX_STORED_MANIFEST_BYTES`, 16 MiB |
| Candidate lookup digest / Mission lookup UUID | 64 / 36 UTF-8 bytes |
| Artifact canonical bytes / content | Existing owner maxima, 4 KiB / 1 MiB |
| Selected Artifact Event | At most one; 64 KiB payload |
| Selected Mission desired outcome and each declaration JSON field | 64 KiB per field |
| Selected Mission admission members | 256; each Resident/configuration identity at most 36 UTF-8 bytes |
| Selected Mission admission Event | At most one; 1 MiB payload |

These are engineering read limits, not model/request/token/budget limits. Exceeding one returns
`ReadLimit`; the code does not silently truncate. Missing selected records still go through their
owning proof rather than receiving an invented empty value.

This is **not a total process, SQLite, or transitive owner-verification allocation bound**. SQLite
can allocate while evaluating these queries. Full manifest reconciliation traverses completion and
other owner dependencies, including historical records and their Events; those existing readers
retain their existing allocation behavior. They can also re-read records independently of the
context's direct selection. The preflights do not cap every projection String allocated by an
existing reader, such as Artifact digest-algorithm or canonicalizer columns; only the selected
fields listed above are covered. The new helpers do not claim to retrofit allocation bounds into those
owners or to bound the aggregate closure. Expanding that guarantee requires a separately scoped
owner-reader change; a cap checked after an unbounded load would not prove it.

## Bounds for the added current facts

Before the new owner reads transfer their protected values, scoped SQL preflights enforce:

- At most 256 lifecycle decisions per selected Resident, 256 configuration decisions and bindings
  per selected Resident, and 256 activation decisions per referenced resource history; no truncation.
- At most seven Capability rows per configuration, checked before the existing owner collects them.
- Resident/decision UUID columns and optional predecessor UUIDs at most 36 UTF-8 bytes; protected
  Resident display/role/lifecycle fields and decision/configuration token fields at most 64 KiB.
- Configuration binding activation/configuration IDs at most 36 bytes and budget tokens at most
  64 KiB. Referenced activation rows are preflighted before the old configuration proof/render reads
  them, including their current resource tails.
- Activation endpoint at most 128 bytes, model name at most 1,024 UTF-8 bytes, digest fields at most
  64 bytes, and each selected feature JSON/provider/state field at most 64 KiB. Both the initial
  bounded lookup and the complete selected resource history receive these checks.

These are engineering limits owned by the read, not resource or request budgets. Matching Event
proofs compare owner-constructed bounded payloads through SQL counts; they do not load stored Event
payload strings into Rust. SQLite evaluation and the older historical candidate/manifest proof
retain the allocation limitations described above. Per-history limits do not establish an aggregate
or total-transitive bound, and unrelated old public row readers keep their original contracts.

## Refusal surface and verification

`CandidateEvaluationError::name()` and `Display` return closed fixed names. Nested account, SQLite
and owner details remain available through `Debug` and `Error::source`; no nested `Display` is
forwarded into the outward message. Public-interface tests use the existing Outcome fixture and
actual owning setup/read services. Raw SQLite access is limited to test accounting and deliberate
corruption of disposable fixture records. Provider inspection in fixture setup uses a bounded local
fixture server; it is not inference or a live resource test.

This is an owner tracer for later context UI and producer work. The test evidence is recorded in the
B15 `CONTEXT-BUILD.md` handoff; no standalone type or green test here establishes a real Resident
Evaluation or an operator-ready screen.
