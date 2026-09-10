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

## What this read does not establish

`current_eligibility()` is explicitly `NotMeasured`: historical admission does not establish current
Resident lifecycle, configuration, capability, activation or request authority. Reconfiguring an
author preserves the historical pair. Forming a later Team cannot repair an old one-member Mission.
The author exclusion is keyed on Resident identity in the implementation. Valid public-owner
history still binds that author to its original admitted configuration, so the reconfiguration
case proves historical preservation, not discrimination against a configuration-keyed filter in
this narrowed tracer. That latter challenge belongs to a future current-eligibility read.

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
