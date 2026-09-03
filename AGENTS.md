# AGENTS.md — Beacon repository contract

## Definition of record

Beacon is one local-first office suite for AI agent teams supervised by one human operator.
Beacon is the canonical project and local-alpha product name. Application IDs, CLI commands, and
package namespaces use Beacon so active work has one identity. Domains, public releases, and other
irreversible brand commitments still wait for the recorded naming and release gates. `beacond` is
the canonical internal supervisor name.

Read these before changing the product:

1. [README.md](README.md) — product vision.
2. [CONTEXT.md](CONTEXT.md) — canonical domain language.
3. [docs/BLUEPRINT.md](docs/BLUEPRINT.md) — Alpha 1 product and technical design.
4. [docs/adr/](docs/adr/) — accepted architectural decisions.
5. [GOVERNANCE.md](GOVERNANCE.md) — project stewardship and maintainer authority.
6. [docs/authority.md](docs/authority.md) — inherited authority findings that the unified
   mechanism must preserve — and its six siblings, one per BLUEPRINT §3 surface, produced the
   same way (re-derived from the legacy trees at pinned revisions, cited, measured; findings, not
   decisions): [briefing](docs/briefing.md) (Athena), [conversations](docs/conversations.md)
   (Buzz), [knowledge](docs/knowledge.md) (Minerva), [models](docs/models.md) (Vulcan),
   [evaluations](docs/evaluations.md) (ReplayDeck — which has no code — plus Icarus's harness),
   [decisions](docs/decisions.md) (Janus's gate model), [execution](docs/execution.md) (Icarus).
   Each ends with open questions that are the Product Steward's, not the page's.

If an older handoff, roadmap section, Python module, or comment describes the project, then called
Mosaic, as a read-only federating desk, it is historical. Do not reconcile the two designs. The
unified Rust product wins.

## Product invariants

- Beacon supports all agent work; software production is Alpha 1's proving workflow, not its
  permanent scope.
- Beacon is one product, one codebase, one authority model, and one managed local store.
- Alpha requires a local Operator Account before Workspace data or Operator Commands are
  accessible. Sign-in is passkey-first with password fallback. Local workflows remain available
  offline after sign-in. Password fallback is same-machine only. Remote access requires a passkey.
- An optional Cloud Link synchronizes only theme, keyboard shortcuts, accessibility preferences,
  and notification preferences in Alpha. It synchronizes no notification contents or history,
  Workspace data, Projects, Missions, Agents, Policy, secrets, or authority.
- The official hosted Cloud Link is the default setup path for broad adoption. An operator-hosted
  endpoint is a first-class option using the same documented protocol and data boundaries. The
  complete local product and self-hosted Cloud Link remain free without a time limit. Official
  hosting is an optional $5-per-month managed-sync service, not a feature-unlock tier. Launch
  regions are the United States and European Union. Version history for synchronized preferences
  remains available locally and when self-hosted; official hosting manages its remote copy. Region
  changes require explicit migration and never occur silently. Self-hosting uses a one-command
  Docker installer for Linux x86-64 and ARM64.
- Local account recovery uses an Operator-held offline Recovery Key and does not depend on Cloud
  Link. Setup cannot complete until the Operator confirms that the Recovery Key was saved. A
  successful rotation immediately invalidates the old key.
- The human session locks after 30 minutes of inactivity by default. The duration is configurable.
  Operating-system lock or suspend locks it immediately. Locking hides Workspace data and blocks
  Operator Commands but does not pause authorized Agent work.
- Multi-factor authentication is optional for same-machine access. Human remote access requires
  local approval of the exact Paired Device through a QR code and human-compared matching code,
  plus a passkey and MFA. Pairing is revocable. The device list shows last use and offers one-action
  revocation. Pairings expire after 90 inactive days, with an Attention Item seven days before
  expiry. Password fallback never authorizes remote access.
- Remote Worker leases use machine identity and do not count as Operator sign-in or device pairing.
- A Project is the long-lived center. A Mission is the finite unit of outcome and accountability.
- Only the Operator admits or retires Resident Agents. Residents form Teams, create Missions,
  assign work, and create bounded Specialists inside operator-authored Policy.
- Fresh installs propose an editable Starter Organization with professional role templates. It
  creates no Resident or authority until the Operator accepts it.
- Track Records are evidence profiles, never composite scores or ambient authority.
- Failed work receives at most one automatic Recovery Attempt before explicit reassignment with a
  Handoff.
- Each Installation is independent and may host multiple isolated Workspaces. A Worker is not a
  federated Installation.
- Resource Presets may be reused across Workspaces but carry no secret, activation, or authority.
- Cross-Workspace work moves only through an explicit Handoff Package that creates no live link or
  inherited authority.
- Budget Envelopes nest from Workspace to Project to Mission to Agent; children cannot exceed
  their parent.
- Discovery grants no capability. Local models and Workers require Operator activation before use.
- Activated local inference is preferred. Cloud Routes require explicit Project permission.
- Material Artifacts require Evaluation by an Agent other than their author.
- Only Attention Items may trigger operating-system notifications.
- Workspace transfer uses an open documented format and excludes secrets. Import restores records
  first; a Reconnect Plan requires explicit Operator confirmation before authority returns.
- Stable is the default update channel; Preview is opt-in. Updates never install during active Runs.
- Encrypted Safety Snapshots must verify before updates and Policy-classified high-impact Runs.
  Default retention keeps the latest ten plus one daily snapshot for 30 days under a visible quota.
- Off-device snapshots are optional, encrypted before transfer, and require no Beacon-hosted
  service.
- Alpha relies on operating-system disk encryption for live Workspace data and never describes the
  open store as Beacon-encrypted. Disabled or unknown status produces a persistent, unobtrusive
  warning but does not block Alpha use. The warning opens an operating-system-specific setup guide
  and status recheck without changing system settings. Portable Safety Snapshots remain
  independently encrypted.
- Private Memory is agent-scoped. Shared Memory is explicitly scoped and provenance-linked.
- Suite apps are lenses over shared objects, not separate state silos.
- Primary navigation is Briefing, Projects, Conversations, Knowledge, and Software. After sign-in,
  every ordinary launch opens Briefing. Operational capability surfaces appear in context instead
  of becoming competing silos.
- Software is the first production application. Research is the second after Software proves the
  complete Alpha loop. Operations is the third.
- Beacon proves one-Operator Workspaces first. Multi-Owner Workspaces are a later product phase and
  cannot weaken the proven authority, attribution, and audit model. Owner is the only human role;
  every Owner uses an individual Operator Account. Every Owner may invite or remove another Owner.
- Research reports use canonical open Markdown with structured citations. Polished PDF is always
  regenerated and never becomes a separate source.
- Briefing acknowledgement advances only through the Operator's explicit **Mark caught up** command.
- Theme choices are Light, Dark, and Follow System. Light is selected on first launch, and all three
  preserve the same semantic meaning.
- Onboarding offers a bundled, disposable Rust task-tracker Git repository as the optional Sample
  Project but cannot start it or spend resources silently.
- Onboarding offers background startup as an opt-in choice. It remains disabled until the Operator
  enables it.
- Every completed Mission publishes a human-readable Outcome Document linked to its Evidence,
  Evaluations, spend, unfinished work, uncertainty, and Receipts.
  Its canonical export is open Markdown. Its regenerated PDF is concise by default and may include
  an optional verification appendix. Policy-classified high-impact Missions include the appendix
  automatically.
- Every core operator workflow is keyboard-complete and targets WCAG 2.2 Level AA through
  automated checks and manual testing with Orca as Alpha's reference Linux screen reader.
- User-facing suite names are professional and functional; legacy mythological names express
  lineage only.
- Permanent names exclude animals and assistant personas. The final brand must be an uncommon real
  word that is easy to pronounce and spell after hearing it once.
- A clean application, CLI, and package namespace is a hard gate for the permanent name.
- Alpha architecture and local identifiers proceed under the Beacon name while public naming
  clearance remains a separate release gate.
- The command palette reaches every app and safe action through the ordinary Command and authority
  path.
- Pause Work is Installation-wide and Operator-only. It blocks new dispatches and lease renewals,
  then requests bounded interruption without claiming uncertain Runs stopped.
- Before first enabling Bounded autonomy in a Workspace, the Operator acknowledges a concise safety
  notice. The notice explains external effects, policy limits, Pause Work, and Receipts. It grants
  no authority, waives no rights, and replaces no Gate.
- Pause Work remains available from the desktop, system tray, and a configurable global shortcut.
  Beacon supplies a platform-appropriate default that the Operator may change or disable. Every
  entry point uses the ordinary Operator Command and authority path.
- Resume Work requires the Operator to review and resolve or explicitly accept every stopping or
  unconfirmed Run.
- Linux Alpha ships AppImage first. A signed native `.deb` follows at beta.
- Rust is the canonical source of domain commands, queries, results, and events.
- The desktop, MCP adapter, and recovery CLI cross the same typed interfaces.
- beacond is the only store writer and the only process that may authorize external effects.
- One beacond control plane may lease Runs to remote Workers; Workers hold no independent authority
  and cannot report success without a verified Receipt.
- Every external effect passes through Execution as a digest-bound Proposal.
- Plugins have no raw store access or ambient authority and use the public typed interfaces.
- Registry Plugins carry publisher signatures and separate registry attestations. Deliberate local
  sideloads receive the same sandbox and require an explicit source, risk, and permission review.
- The complete local product and self-hosted Cloud Link remain free without a time limit; the
  optional $5-per-month official service sells managed sync convenience, not withheld capability.
- Evidence informs authority but never grants it.
- Material Proposal drift invalidates a Ruling.
- Terminal, absent, broken, and not evaluated are distinct states.
- No model, provider, check, or tool fallback is silent.
- Every material Run terminates with an honest Receipt, including interrupted and failed Runs.
- Secrets live in the operating system's secret store and never enter project records or logs.
- Diagnostic collection is off by default and remains local. Alpha feedback uses a locally
  generated scrubbed bundle that the Operator previews and exports manually. Beacon never submits
  it automatically.
- Initial releases are invite-only technical alphas.

## Module design

Build deep capability modules behind small interfaces:

- Conversation
- Work
- Evidence
- Inference
- Evaluation
- Authority
- Execution

The interface is the test surface. UI apps do not join module tables, rebuild domain objects field
by field, or encode domain policy. Introduce an adapter only where at least two implementations are
real, normally production and test.

The shared kernel stays small: identities, time, Project and Mission references, Artifact identity,
Provenance, typed relations, measurements, cursors, and error envelopes. Capability-specific
statuses remain capability-specific closed vocabularies.

## Authority and safety

Alpha 1 defaults Projects to Supervised autonomy. Bounded autonomy is allowed only for disposable
local repositories until the Alpha acceptance criteria pass.

Human review is always required before:

- production deployment or public communication;
- deletion or rewrite of shared state;
- infrastructure replacement;
- schema migration against live data;
- authentication, session, secret-handling, financial, or legal changes;
- credential, network, or budget expansion;
- policy changes intended to authorize the pending action;
- effects Beacon cannot roll back or prove.

Repository creation, remote publication, git push, live-system migration, and retiring any legacy
system require Kevin's explicit approval.

## Persistence rules

SQLite uses plain SQL, foreign keys, explicit migrations, and reconciliation checks.

- Meaningful nulls and defaults are forbidden; explicit states carry meaning.
- Every type field, persisted column, and enum member is reconciled in both directions.
- Imported source words survive every mapping.
- Digests carry their algorithm and canonicalizer identity.
- Timestamps carry normalized instants and source encodings.
- Structural belonging and access scope are separate relations.
- Clock-derived and cross-table-derived verdicts are computed at read time.
- References retain their source triple after resolution.
- Accepted Commands write their domain change and audit event atomically.

## Legacy Python tree

The Python desk is validated research, not the product trunk. Its contracts, adapters, renderers,
and store experiments preserve migration knowledge and proven failure modes. Do not extend them
with native Beacon features. Migration readers remain read-only, one-shot, provenance-preserving,
and fail closed against live or unverifiable source databases.

Establish the Rust workspace beside the Python tree. Preserve the historical Python implementation
on a tagged legacy branch before removal or relocation.

## Working agreement

- Work in your own clone or worktree.
- Declare paths before editing and avoid whole-tree formatting across another lane's work.
- Read immediate callers, exports, shared types, and tests before changing a module.
- Prefer the smallest vertical slice that crosses the real interface.
- Test observable behavior through Command, Query, and Event interfaces.
- For a bug guard, prove the test fails against the plausible wrong answer.
- Run the full relevant gate after the last edit and read its complete output.
- Do not claim operability from a green unit suite; run the product path being claimed.
- Do not commit, push, deploy, migrate live data, or retire old systems without the gates above.

## Current state

The expanded Alpha blueprint is accepted design. Phase 0 has begun beside the preserved Beacon
Legacy Desk. The Rust workspace contains the `beacond` supervisor, managed SQLite store, local
Owner account and Recovery Key boundary, authenticated local transport, configurable human-session
lock, Linux screen-lock/suspend monitor, Tauri/React shell, and the authenticated Starter
Organization decision. Accepting its editable Lead, Builder, Reviewer, and Researcher proposal
creates inactive Resident identities only; it grants no model, capability, budget, activation, or
execution authority. The Owner can activate, suspend, reactivate, and permanently retire those
identities through an append-only, stale-safe lifecycle; availability still grants no execution
authority. Authenticated Owners can create and inspect Projects with immutable, contiguous
Charter revisions; the current Charter pointer must identify the terminal revision, and creation
and revision remain limited to Observe or Supervised autonomy with no spending or external effects.
The Owner can publish a new immutable revision from the exact current revision and inspect the full
history. Briefing can discover installed Ollama models through a bounded loopback read, but every
result remains ephemeral and inactive: discovery persists nothing, invokes nothing, and grants no
capability or authority. The Owner can separately inspect an exact local model, approve a known
Provider Feature subset, activate it as revocable Workspace eligibility, and deactivate it without
running the model or granting Agent authority. Inside one Project, the authenticated Owner can form
an immutable named Team from the exact current configurations of one or more eligible Residents.
Formation is truthfully Operator-attributed, keeps later eligibility separate from formation
evidence, assigns no permanent lead, and creates no Mission or authority. The Owner can admit a
finite Mission against the exact current Team evidence, and can bind a Project to one immutable
Repository revision naming a source, a pinned base revision, and its Declared Checks; binding is a
declaration that opens nothing and runs nothing.

Execution exists as far as running a Project's own Declared Checks and no further. Beacon detects
whether this host can contain a Run by building a Landlock ruleset, applying it in a child, and
reporting only what that child observed; a sandbox it has not demonstrated is never reported as
one. Check preflight re-verifies a binding against the current Charter, the repository, the pinned
revision's presence in the object store, and each check's resolved working directory, then emits a
Receipt that names what it did not verify. An admitted execution runs those checks inside a
disposable Git worktree at the pinned revision, bounded in wall-clock, CPU, address space, file
size, and captured output, interruptible, with the environment cleared and filesystem writes
confined to the worktree. Every Run carries a lease and reaches a terminal Receipt or is recorded
`unconfirmed`; a worktree that refuses removal is reported rather than overridden.

The authenticated Owner can opt in to install the bundled
[Sample Project](docs/implementation/sample-project-installation.md). It installs inert Project
and repository state and runs nothing. The installation creates no Run, Receipt, Proposal, Ruling,
model request, network request, or Declared Check execution, and grants no authority.

An Owner can also submit an immutable Proposal, bound to the exact canonical bytes Beacon stores and
identified by their digest: it records intended effects, inputs, bounds, checks, and budget for
review, is backend-only and inert, and creates no Gate, Ruling, worktree, process, Invocation, or
dispatch of any kind.

The Owner can then record a Ruling against one exact Proposal digest, copying the Charter, Policy,
and Repository revisions and the pinned base revision it was decided under, append-only and with a
reversal recorded as a new Ruling citing the prior one rather than as an edit; it is backend-only,
it reports drift and supersession as facts rather than as a verdict, and it creates no execution
authority, because a Ruling is evidence that a person decided and there is no dispatch to consume
it. An authenticated Owner can also record one immutable Candidate per Proposal: the SHA-256 of
each declared mutation's file bytes before and after, with the kind and path copied from the
verified Proposal. It selects bytes, creates no authority, performs no repository action, and is
refused if a second one is recorded for the same Proposal. A second Ruling grammar,
`beacon.ruling.v2`, records a decision that names the Candidate it was shown: it is recorded only
through a presentation of that exact change, v1 Rulings remain valid and simply name no Candidate,
and neither grammar dispatches anything, because no dispatch exists.

Resident-issued Team formation, Conversations, Gates, Evaluations, model Invocations,
Cloud Link, passkeys, active agent work, and a downloadable installer do not exist yet. Nothing an
Agent authored has ever been executed: the only commands Beacon runs are the Declared Checks an
Owner bound by hand. Do not describe this checkpoint as a usable Alpha.
