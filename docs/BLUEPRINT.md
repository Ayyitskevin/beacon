# Beacon Alpha Blueprint

## 1. Product definition

Beacon is a local-first office suite for AI agent teams. One operator supervises many agents
performing knowledge work and software production. The suite owns the full loop from discussion
through execution and receipt.

Beacon absorbs the capabilities of Athena, Buzz, Minerva, Vulcan, ReplayDeck, Janus, and Icarus.
Those systems are design sources and migration sources, not runtime dependencies. Beacon is one
product, one codebase, one authority model, and one local store.

Beacon is not limited to software. Its objects and interfaces must also support research,
operations, media, analysis, and new forms of agent work. Software production is Alpha 1's
high-rigor proving ground because it makes effects, evaluation, rollback, and receipts concrete.
The product then grows into a broad family of first-party and extended applications over the same
Workspace, like an office suite for Agents rather than a permanent coding-only tool.

The destination is a complete standalone, installable, open-source product.

Beacon is the temporary working title while Alpha architecture proceeds. Preliminary screening
rejected it as a permanent candidate because current AI-agent products already use the name. The
working title does not authorize a Beacon application ID, CLI command, package namespace, domain,
or public release.

The permanent name should be an uncommon real word that evokes a durable knowledge institution
rather than an operations center, Agent, model, assistant persona, temporary workshop, or animal.
It must be easy to pronounce and spell after hearing it once. Its character is professional with a
quiet academic edge: thoughtful and enduring, not governmental or corporate. A clean application,
CLI, and package namespace is a hard gate before selection.

The product is built around two nested objects:

- A **Project** is the long-lived home for purpose, context, policy, teams, and artifacts.
- A **Mission** is a finite outcome inside a Project with ownership and success conditions.

All suite apps are lenses over those same objects. They do not maintain parallel copies.

## 2. Users and operating model

### Primary user

Alpha 1 serves one human operator overseeing one or more autonomous agent teams. It does not
attempt multi-user enterprise collaboration. Beacon is primarily a workplace for Agents; the human
desktop is a calm supervisory window into that workplace. Agents are first-class users of the
system, but they operate through scoped machine interfaces rather than the human desktop.

The solo-operator product must become stable and prove its authority boundaries before Beacon adds
multi-Owner Workspaces. Owner is the only human role in that later phase. Every Owner uses an
individual Operator Account. Every Owner may invite or remove another Owner. Shared ownership never
becomes shared identity or ambient authority.

The human creates and signs in to a local Operator Account before entering any Workspace or issuing
an Operator Command. Sign-in is passkey-first. Password fallback is available on the same machine
only and cannot authorize remote access. The Account remains the local identity source when the
network is unavailable. Setup generates an offline Recovery Key and does not complete until the
Operator confirms that it was saved. The confirmation records the Operator's assertion; Beacon
cannot prove that a separate copy exists. A successful Recovery Key rotation immediately
invalidates the old key. Recovery does not depend on Cloud Link.

The human session locks after 30 minutes of inactivity by default; the Operator may change the
duration. Operating-system screen lock or suspend locks the Beacon session immediately. Session
Lock hides Workspace data and blocks Operator Commands without pausing already authorized Agent
work.

Multi-factor authentication is optional for same-machine access. Human remote access requires MFA
and a passkey, plus local approval of the exact Paired Device. Pairing presents a QR code and a
short matching code that the human compares before approval. The paired-device list shows each
device's last use and provides one-action revocation. Pairings expire after 90 days without use.
Seven days before expiry, Beacon creates an Attention Item so the Operator can use, revoke, or renew
the pairing. Password fallback does not authorize remote access. Remote Worker leases use machine
identity and are not Operator sign-in or device pairing.

An optional Cloud Link synchronizes only theme, keyboard shortcuts, accessibility preferences, and
notification preferences in Alpha. Notification contents, events, and history never synchronize.
Neither do Workspace data, Projects, Missions, Agents, Policy, secrets, credentials, Routes,
budgets, capabilities, Rulings, or execution authority. The official hosted service is the default
setup path because it requires the least configuration and can provide consistent cross-device
operation. Operators may instead connect a self-hosted endpoint implementing the same documented
protocol and data boundaries. The local product and protocol-compatible self-hosted Cloud Link
remain free without a time limit. Official hosting is an optional $5-per-month convenience for
people who want managed sync without server administration. It requires the Operator to choose the
United States or European Union data region at launch. Version history for synchronized preferences
remains available locally and when self-hosted; the hosted service manages its remote copy. A Linked
Device synchronizes allowed preferences but receives no remote access or authority. Changing regions
is an explicit migration and never silently relocates data. Self-hosting ships through a one-command
Docker installer for Linux x86-64 and ARM64. Local identity and the complete local product depend on
neither option.

The product model stays domain-neutral. A Project may point at a Git repository, a research
question, an operations runbook, a media production, or another collection of Artifacts.

### First-run organization

A fresh Installation proposes an editable Starter Organization with Lead, Builder, Reviewer, and
Researcher role templates. It also discovers compatible local models and Workers. Discovery creates
inactive candidates and grants no capability. The Operator may accept, edit, or discard the proposed
organization, then reviews each Resident's model binding, permissions, and budget before admission.
Agent Roles remain professional and visible. Personal Display Names are optional and never
substitute for Agent identity, role, capability, or authority.

This avoids both failure modes: a blank expert-only setup and a canned workforce that becomes active
without its Operator choosing it.

After admission, onboarding offers a bundled, disposable Rust task-tracker Git repository as the
optional Sample Project. It demonstrates the complete software-factory loop before the Operator
connects real work. It creates no Run, model spend, network use, or external effect until the
Operator explicitly starts it. Onboarding also offers background startup as an opt-in choice. The
supervisor does not start with the operating-system login until the Operator enables it.

### Division of responsibility

The operator:

- creates Projects and states desired outcomes, constraints, Policies, and hard limits;
- checks Briefings and contributes context or judgment when useful;
- supplies or authorizes model credentials;
- rules on exceptional or high-risk Proposals;
- interrupts work, changes the operating envelope, and reviews Receipts.

Agents:

- propose and create Missions aligned with the Project Charter and within Project Policy;
- form Teams, decompose outcomes, and assign work among themselves;
- create Specialist Agents within delegated capability, time, and budget ceilings;
- maintain their own desks, coordinate through Threads, and share necessary knowledge;
- create Claims, Evidence, Artifacts, Evaluations, and Proposals;
- invoke models and execute only through bounded interfaces;
- escalate honest blockers and leave durable Receipts.

The core relationship is stewardship rather than dispatch. The operator defines and can change the
envelope; Resident Agents run the workplace inside it; Beacon stops and asks only when policy,
risk, uncertainty, or authority requires human judgment.

Teams may contain both Resident Agents and Specialist Agents. Only the Operator may admit,
activate, suspend, or retire a Resident Agent. Agent Packs may propose Resident configurations but
cannot install them. Resident Agents keep a durable identity, role, history, and Track Record
across Missions. Specialist Agents receive a narrow Mission-scoped identity, capability set,
context packet, and budget. Residents may spawn and close Specialists without asking when Policy
already delegates those bounds. Neither Resident nor Specialist Agents may expand their own
capability, credential, network, budget, or policy ceilings.
A Specialist's active identity ends with its Assignment, but its activity and Receipts remain
attributable forever.

### Project Charter and self-directed Missions

Every Project begins with an Operator-authored Project Charter. The Charter states purpose, desired
outcomes, constraints, prohibitions, budget boundaries, and the authority envelope. Resident Agents
may create Missions without operator dispatch when the proposed outcome is traceably aligned with
that Charter and passes Project Policy. They may not rewrite the Charter to admit their own work.
Ambiguous alignment becomes an Attention Item rather than being silently assumed.

### Team leadership

Policy selects a Team Lead from eligible Resident Agents using declared capability fit,
availability, relevant Track Record, and current conflicts. A Track Record is an evidence profile,
not a composite score or leaderboard. Selection criteria query named evidence and thresholds. The
decision and its evidence are attributable. No Agent becomes Lead through seniority alone.
Leadership grants coordination responsibility rather than extra execution authority. The Operator
may replace the Lead at any time.

## 3. The suite

The primary navigation contains five applications: Briefing, Projects, Conversations, Knowledge,
and Software. Supporting capabilities appear inside those applications and through deliberate
drill-down rather than competing for permanent navigation space.

### Briefing

Briefing is the operator's supervisory surface, descended from Athena. It shows the Project
portfolio, active Missions, Team health, spend, blocked work, recent receipts, and Attention Items.
It optimizes for exceptions, not activity volume.

### Projects

Projects holds purpose, success conditions, repositories, knowledge, teams, policy, budgets, and
Mission history. A Project can contain many Missions, but a Mission has one Project and one
accountable Team.

Every completed Mission publishes a human-readable Outcome Document. It summarizes the result
against success conditions, delivered Artifacts, Evidence, Evaluations, actual spend, unfinished
work, uncertainty, and linked Receipts. It is a durable Artifact, not a replacement for a Receipt.
Open Markdown is its canonical export. A PDF is regenerated from that Markdown and never becomes a
separate source of truth. The PDF is concise by default and may include an optional verification
appendix with the same linked Evidence, Evaluations, and Receipts.

### Conversations

Conversations is the coordination surface descended from Buzz. Threads may attach to Projects,
Missions, Claims, Artifacts, Proposals, or Runs. Messages carry actor identity, reply structure,
timestamps, and optional signatures. A conversation can produce an Artifact or Mission through an
explicit command; nothing becomes authoritative merely because it was said.

### Knowledge

Knowledge is the research, memory, and claim surface descended from Minerva. Claims are separate
from their support. Evidence identifies exact source spans or content digests. Findings record
support, contradiction, uncertainty, and supersession. A model summary never erases the underlying
source.

### Software

Software is the first production application. It turns a feature brief for an existing repository
into a traceable Mission, plan, research, candidate diff, independent Evaluation, Ruling, bounded
Run, and Receipt. It composes the deeper inference, evaluation, authority, and execution
capabilities without making the product permanently coding-only.

After Software proves the complete Alpha loop, Research becomes the second production application.
It turns a question into a research plan, source corpus, Claims, Evidence, independent Evaluation,
and a provenance-linked report. Open Markdown and a structured citation manifest are the canonical
output. A polished PDF is regenerated from them and is never edited as a separate source or kept as
the only copy. Research joins primary navigation only when that full workflow works; the existing
Knowledge application remains the durable evidence and memory surface beneath it.

After Research proves that workflow, Operations becomes the third production application. It turns
runbooks, recurring procedures, fleet health, exceptions, and verified interventions into Missions,
Attention Items, Proposals, and Receipts without creating a second authority path.

### Supporting capability surfaces

#### Models

Models is the inference surface descended from Vulcan. It manages provider connections, Routes,
model eligibility, privacy constraints, budgets, live usage, and the replayable Invocation ledger.
Activated local inference is preferred by default. A discovered local model is ineligible until
the Operator activates it. A cloud provider, Route, and permitted data scope must be enabled
explicitly by Project Policy before use. Every Invocation is attributed to an Agent,
Project, Mission, Route, provider, and model. A failed preferred model never causes a silent
fallback.

#### Evaluations

Evaluations is the quality surface descended from ReplayDeck. It manages fixtures, scenarios,
rubrics, deterministic checks, model judges, baselines, and Replays. Outcomes distinguish
reproduced, diverged, not replayable, and not attempted. A green result names the exact Artifact and
environment it evaluated.

#### Decisions

Decisions is the authority surface descended from Janus. It shows what needs a Ruling, who may
decide, what exact Proposal is bound, whether that Binding still holds, and whether the Ruling
remains in force. A Ruling applies only to its digest and policy version. Any material change
produces a new Proposal and invalidates the old Ruling.

#### Execution

Execution is the software-factory and action surface descended from Icarus. It turns approved
Proposals into sandboxed Runs, streams progress, enforces ceilings, supports interruption, performs
declared checks, and emits Receipts. It is the only module allowed to cause external effects.

#### Activity

Activity is the shared flight recorder. It answers who did what, when, for which Mission, under
which policy, using which model and budget, with which result. It is not a second source of domain
truth; it is an immutable audit projection of domain changes.

#### Extensions

Extensions manages installed Agent Packs, Connectors, Views, Evaluators, and Worker Capabilities.
It shows version, publisher, compatibility, requested permissions, integrity, update status, and
recent activity. An extension can add capability through public interfaces; it cannot replace
identity, Policy, authority, the event ledger, or Receipt verification.

## 4. Interaction model

### Human desktop

The desktop uses a stable shell:

- left rail: suite apps and saved Project scopes;
- top bar: current Project, Mission, search, and command palette;
- center: the active app;
- right inspector: selected object's provenance, relations, authority, and history;
- status strip: active Runs, model health, spend, connection state, and **Pause Work**;
- Attention drawer: only items with a named actor and concrete next action.

The command palette reaches every app, canonical object, and safe action. It preserves the current
Workspace, Project, and Mission scope. Palette actions use the same Commands, Policy, Gates, and
Receipts as every other caller; discoverability never becomes a privileged execution path.

After successful sign-in, every ordinary application launch opens Briefing. An authenticated deep
link may open its named object directly. The default Briefing contains two sections:
**Since your last visit** and **Needs attention now**.
The first summarizes material changes after the last Briefing cursor the Operator acknowledged.
The second shows current actionable exceptions even when they began earlier. Background refresh
and opening the Briefing never advance the acknowledged cursor. Only an explicit **Mark caught up**
command does. Dense operational detail appears in an Operations view and object inspectors rather
than occupying every screen. Density is a tool the operator invokes, not the product's permanent
emotional state. User-facing labels stay functional and professional; the historical project names
remain lineage, not product navigation.

The visual system is editorial and quietly academic. Its choices are Light, Dark, and Follow
System, with Light selected on first launch. Beacon remembers an explicit choice; Follow System
tracks the operating-system preference. Every mode preserves information hierarchy, readable
contrast, focus visibility, and the same semantic status colors.

The human desktop targets [WCAG 2.2 Level AA](https://www.w3.org/TR/WCAG22/) and supports every
core workflow by keyboard. Focus order, visible focus, semantic names, reduced motion, text scaling,
and screen-reader output are acceptance requirements rather than optional polish.
Acceptance combines automated audits with manual keyboard and screen-reader testing on a supported
Linux configuration. Orca is Alpha's reference screen reader. Neither form of testing substitutes
for the other.

Every object has one canonical deep link. Switching apps preserves the selected Project, Mission,
and object. The operator can move from a failing Evaluation to its Artifact, Claim, Thread,
Proposal, Gate, or Run without searching by copied identifiers.

### Progressive transparency

Beacon records enough structured Trace data to explain work without making every screen a terminal.
Summaries are the default. The operator can drill into source Evidence, full prompts and model
responses, tool requests and results, policy classifications, Evaluation inputs, diffs, and
Receipts.

Transparency means observable inputs, outputs, decisions, and effects. Beacon does not claim to
expose hidden model reasoning that a provider did not return. Secret values are scrubbed before
persistence, and Trace retention is an operator policy.

### Agent desk

Each Agent receives a bounded desk packet containing:

- identity, Team, role, and current Assignment;
- Mission purpose and success conditions;
- permitted Capabilities, repositories, Routes, and budgets;
- unread directed messages and operator instructions;
- relevant Claims, Evidence, Artifacts, and prior Receipts;
- current blockers and the allowed commands that can resolve them.

The packet is a projection, not a prompt transcript. It is versioned and can be re-issued without
silently changing the Mission.

### Memory model

Each Agent has Private Memory for its own working continuity. Other Agents cannot read it by
default. The Operator can inspect it only through deliberate drill-down with access recorded in
Activity. Teams also use Shared Memory scoped to a Workspace, Project, Mission, or Team.

Moving knowledge from private to shared is an explicit Memory Promotion that creates a new,
provenance-linked record and preserves the source. Raw Trace is not automatically Memory.
Retention, summarization, and promotion are separate policy decisions. When a Specialist closes,
its private working memory is archived under retention Policy, necessary handoff knowledge is
promoted explicitly, and its Artifacts, events, and Receipts remain attributable.

### Attention semantics

Only an Attention Item may request an operating-system notification. Activity, messages, completed
work, background health, and ordinary status changes remain inside Beacon unless they create an
actionable Attention Item. Notification preferences may narrow interruptions further but cannot
make a non-actionable event interrupt the Operator.

An Attention Item exists only when all three are true:

1. a condition is unresolved;
2. a named actor is eligible to act;
3. the product can state the concrete next action.

Routine success, background activity, enforced refusals, and not-yet-run checks remain visible in
Activity but do not pollute the operator's attention queue.

Routine work does not pause merely because time has passed. Beacon refreshes the Briefing and
creates Attention Items when risk changes, spend crosses a Policy threshold, forecast spend may
breach a ceiling, work stalls, an Evaluation fails, authority becomes ambiguous, Worker health
degrades, or the Operator asks. Time schedules may refresh a Briefing, but they gate work only when
the Project Charter or Policy explicitly says they do.

### Failure recovery

Eligible transient failure receives one automatic Recovery Attempt under the same Proposal,
authority bindings, Assignment, and remaining Budget Envelope. Beacon records it as a distinct
attempt. A second failure pauses that Assignment and reassigns it to an eligible Agent with an
explicit Handoff, or creates an Attention Item when no safe reassignment exists. Recovery never
widens authority, refreshes budget, hides the failed attempt, or becomes an unlimited retry loop.

### Pause Work

The Operator can place the entire Installation into **Pause Work**. beacond records the paused
state before sending interruption requests. While paused, Beacon rejects new Proposal dispatches
and new or renewed Worker leases. Active Runs receive a bounded interruption request.

A local or remote Run remains stopping or unconfirmed until a terminal Receipt or lease expiry
proves its state. Pause Work does not undo effects, revoke Rulings, or shut down the supervisor.
Only the Operator may resume work; an Agent, Plugin, Worker, schedule, or reconnect cannot.

Before **Resume Work**, the Operator reviews every stopping or unconfirmed Run. The review shows
its Worker, last contact, lease expiry, latest Receipt, known effects, and residual uncertainty.
The Operator must resolve the Run or explicitly accept its uncertainty. Restart and reconnect never
resume work automatically.

Pause Work remains available in the desktop, from the system tray, and through a configurable
global shortcut when the main window is closed. Every entry point sends the same attributable,
Operator-only Command to beacond. Closing the main window neither stops the supervisor nor creates
a second authority path. Beacon supplies a platform-appropriate default shortcut that the Operator
may change or disable.

## 5. Authority model

Beacon uses hybrid authority: policy enforcement for routine bounded work, human judgment for
exceptions and high-risk effects.

### Autonomy Profiles

Each Project selects one profile:

- **Observe**: Agents may read, discuss, research, invoke approved models, and evaluate. No external
  mutation is allowed.
- **Supervised**: Agents may prepare and evaluate Proposals. Every external mutation requires an
  operator Ruling.
- **Bounded**: Beacon may execute reversible actions that satisfy an explicit Policy, target
  allowlist, sandbox, test requirement, and budget. Everything else creates a Gate.

Alpha 1 defaults to Supervised. Bounded is available only for disposable local repositories.
Before first enabling Bounded autonomy in a Workspace, the Operator acknowledges a concise safety
notice covering possible external effects, the limits of Policy, Pause Work, and Receipt review.
This acknowledgement is education, not authority or a waiver, and it replaces no Gate.

### Actions that always gate

No Autonomy Profile bypasses the operator for:

- production deployment or public communication;
- deletion or rewrite of shared state;
- infrastructure replacement;
- schema migration against live data;
- authentication, session, secret-handling, money, or legal logic;
- an increase in credential scope, network reach, or budget ceiling;
- a change to the policy that would authorize the pending action;
- any effect Beacon cannot roll back or prove.

### Nested Budget Envelopes

Budget Envelopes form a strict hierarchy: Workspace, Project, Mission, then Agent. Every child
allocation fits within its parent's remaining ceiling, and actual use rolls up through every
ancestor. An Agent cannot increase, transfer, borrow, or reinterpret an Envelope. Unused capacity
remains controlled by the parent scope. Policy may require reservations before concurrent work so
parallel Agents cannot collectively overspend.

### One mutation path

All external effects pass through Execution. Desktop commands, agent commands, Plugins, and future
automations cannot mutate repositories, processes, or networks directly. They submit Proposals.
Execution accepts only a Proposal whose digest, policy version, target set, base revision, checks,
sandbox, model/provider constraints, and budget still match.

Evidence can influence a Ruling but cannot grant authority. An Agent cannot approve its own
Proposal through a model judgment. A Ruling record is not assumed favourable merely because it
exists.

Policy classifies when a Proposal is high-impact enough to require a Safety Snapshot before
dispatch. The snapshot covers Beacon-owned Workspace state and the Proposal's declared mutable
targets that Beacon can restore. If a required snapshot cannot be created and verified, dispatch
fails closed. External effects Beacon cannot capture remain explicitly non-restorable and gated;
a snapshot never turns them into reversible work.

### Authority state

Beacon preserves distinct axes:

- **decider**: operator, policy, or agent proposal;
- **binding**: bytes, intent, handle, step, or unbound;
- **check**: verified, drifted, unverifiable, unchecked, or indeterminate;
- **enforcement**: enforced, reported, or none;
- **lifecycle**: proposed, reviewed, authorized, executed, verified, refused, withdrawn, expired,
  or superseded;
- **force**: pending, in force, spent, or indeterminate.

These values are never compressed into a generic Approved state.

## 6. Technical architecture

### Chosen stack

- Rust workspace for the trusted core, supervisor, store, policy, execution, and machine interface.
- Tauri v2 desktop shell.
- TypeScript and React for the human interface.
- SQLite through plain SQL and Rust-owned connections; no ORM.
- Bundled SQLite with WAL, foreign keys, explicit migrations, and integrity checks.
- Linux first. Other desktop platforms follow after the authority and execution model is proven.
- Linux Alpha ships as an AppImage first. A native `.deb` package follows at beta.

### Workspace isolation

One Installation may host several Workspaces. A Workspace is the isolation boundary for Agents,
Projects, Policy, budgets, credential references, memory, Artifacts, events, Plugin grants, and
authority. Every Command and Query carries one Workspace identity, and beacond authorizes it before
resolving any other object. Cross-Workspace visibility, spending, execution, and reuse are denied by
default.

The Installation owns discovery, updates, diagnostics, and storage maintenance. It may suggest the
same compatible local resource to several Workspaces, but activation, capability, data scope, and
budget are reviewed independently in each Workspace. A Worker serving more than one Workspace
receives distinct leases and cannot move state or authority between them.

The Operator may save a non-secret Resource Preset and apply it to another Workspace. Applying the
Preset creates reviewable suggestions only. It never copies credentials, activates a resource, or
transfers capability, budget, Policy, or authority.

Selected work crosses between Workspaces only through an immutable Handoff Package. The destination
verifies it as untrusted input and explicitly adopts new local records with preserved provenance.
No secret, identity, Policy, budget, authority, live reference, or continuing link crosses with it.

### Process shape

Beacon ships as one product with multiple cooperating binaries from one Rust workspace:

    ┌──────────────────────┐      ┌──────────────────────┐
    │ Tauri desktop        │      │ Agent adapters       │
    │ React human UI       │      │ MCP + recovery CLI   │
    └──────────┬───────────┘      └──────────┬───────────┘
               │ typed commands/queries       │
               └──────────────┬───────────────┘
                              ▼
                     ┌───────────────────┐
                     │ beacond           │
                     │ local supervisor  │
                     ├───────────────────┤
                     │ command interface │
                     │ query interface   │
                     │ event feed        │
                     ├───────────────────┤
                     │ policy + identity │
                     │ seven modules     │
                     │ scheduler + runs  │
                     └─────────┬─────────┘
                               ▼
                     ┌───────────────────┐
                     │ managed local     │
                     │ SQLite + artifacts│
                     └───────────────────┘

beacond is the single writer and authority process. The desktop never owns a database handle.
Agents never receive one. Local clients connect over an authenticated Unix-domain socket. Tauri
starts or reconnects to the supervisor; closing a window does not corrupt or orphan an active Run.
Operating-system-login startup is optional and disabled until the Operator selects it during
onboarding or Settings.

### Diagnostics and telemetry

Diagnostic collection is off by default. Health, events, and troubleshooting data remain local and
inspectable. Alpha feedback uses a locally generated scrubbed support bundle. The Operator previews
and exports it manually, then chooses how to share it outside Beacon. Beacon never submits the
bundle or other diagnostics automatically.

### One control plane, remote workers

Beacon may discover a compatible local or remote Worker, but discovery leaves it inactive. The
Operator must activate the Worker and approve its capability advertisement before it can receive a
lease.

One operator workstation hosts the authoritative beacond control plane, store, policy engine, and
operator desktop. Fleet machines run a small Beacon Worker, not another complete Beacon instance.
A Worker can advertise resources and execute a leased Run, but it cannot create policy, grant
authority, rewrite Project truth, or approve its own work.

Each remote Run uses a signed, time-limited lease bound to:

- Worker identity and eligible capabilities;
- Proposal and policy digests;
- declared Artifact inputs and target outputs;
- sandbox, network, command, time, and budget ceilings;
- required progress and Receipt checkpoints.

A disconnected Worker cannot obtain new work. An active Run may continue only within its unexpired
lease and must buffer a signed partial Receipt. Expiry stops new effects. Reconnection reconciles
events by cursor and submits the Receipt for control-plane verification before the Run can be
called successful.

Alpha 1 implements the same Worker interface with a local adapter. Remote transport arrives after
the local authority and Receipt contracts pass, so distributed execution does not invent a second
path around them.

Each Beacon Installation remains an independent authority and data domain. A remote Worker belongs
to one control plane for each lease; it is not another Installation. Live cross-Installation
synchronization, shared authority, and federation are excluded until they receive a separate design.
Explicit export, import, or Handoff packages are the only planned exchange boundary.

### External interface

All callers use three small interfaces:

- **Command**: request a state change or Proposal and receive an outcome.
- **Query**: request an authorized projection and receive data plus a consistency cursor.
- **Event feed**: subscribe from a cursor to authorized events.

Rust defines the canonical command, query, result, and event schemas. TypeScript types and the MCP
tool schema are generated from them. Callers do not reconstruct domain types field by field.

The first adapters are:

- Tauri IPC for the desktop;
- MCP for agent harnesses;
- a narrow CLI for recovery, diagnostics, and scripted verification.

The Worker protocol is an internal execution adapter at the Execution module's seam. Local and
remote Workers satisfy the same lease-and-receipt interface. Transport does not receive a generic
command channel.

Each adapter authenticates an actor and supplies the same command interface. There is no privileged
UI-only mutation path.

### Extension boundary

Beacon supports five extension classes: Agent Packs, Connectors, Views, Evaluators, and Worker
Capabilities. Declarative Agent Packs are the first public extension type because they can prove
identity, instructions, tool requests, Evaluations, and Policy interaction without introducing a
new transport or ambient execution path. Every Plugin has a versioned manifest declaring
compatibility, settings, commands or views, required Capabilities, data scopes, network
destinations, and integrity metadata.

Plugins never receive raw SQLite access or load arbitrary native Rust into beacond. Untrusted code
runs out of process or in a sandboxed portable runtime and communicates through the public Command,
Query, Event, or Worker lease interfaces. Plugin install and permission expansion require operator
review; ordinary use remains subject to the same Policy, Proposal, and Receipt rules as built-in
features. Packages are pinned by digest.

Registry packages carry a publisher signature plus a separate registry attestation. The signature
proves publisher and package identity. The attestation records the manifest integrity,
compatibility, and validation results the registry checked. Neither proves safety.

The Operator may deliberately sideload a local package after reviewing the same manifest plus a
clear source and risk warning. A sideload has no registry attestation. Sideloading never weakens the
sandbox, grants broader interfaces, or enables automatic updates. Alpha 1 defines and tests this
seam but does not ship a third-party runtime or marketplace.

### Independent evaluation

Every Material Artifact receives an Evaluation owned by an Agent other than its author before it
can support a Ruling, authorize effects, claim Mission success, or become a durable shared
deliverable. Deterministic checks remain valuable evidence but do not erase this separation.
Evaluator identity, criteria, inputs, findings, and conflicts are recorded. The evaluator may
reject, request revision, or report uncertainty, but cannot grant execution authority.

### Deep capability modules

The core contains seven deep modules:

| Module | Owns | Small external interface |
|---|---|---|
| Conversation | Threads, messages, mentions, directed inbox | post, reply, read thread |
| Work | Projects, Charters, Missions, assignments, Track Records, attention | create/advance Mission, form Team, read desk |
| Evidence | Claims, Evidence, Findings, source spans | record claim, attach evidence, evaluate claim |
| Inference | Routes, Invocations, usage, provider health | invoke, inspect route, read usage |
| Evaluation | suites, cases, Replays, outcomes | run evaluation, compare, inspect result |
| Authority | Proposals, Gates, Rulings, Bindings, force | submit, rule, verify binding |
| Execution | sandboxes, Runs, effects, Receipts | prepare, dispatch, interrupt, inspect receipt |

Modules collaborate through internal domain events and typed calls. Their storage is private to
their implementation even though it lives in one database. UI apps never join module tables or
encode business rules.

### Shared kernel

Only these concepts are shared across modules:

- opaque identifiers and timestamps;
- Actor and Capability identity;
- Project and Mission references;
- Artifact identity and Provenance;
- typed relations;
- money, token, and duration measurements;
- event cursors and error envelopes.

Capability-specific statuses remain capability-specific closed vocabularies. The shared kernel
does not invent one universal lifecycle for every object.

## 7. Store and integrity

### One managed store

Beacon owns one local data directory:

- SQLite stores domain records, relationships, policy, indexes, and the event ledger.
- Small content may live in SQLite.
- Large Artifacts live in a content-addressed blob directory managed by Beacon and referenced by
  digest from SQLite.
- External Git repositories remain Git repositories; Beacon records paths, revisions, worktrees,
  and content identities rather than copying their hidden state into the database.

### Persistence rules

- Meaningful absence is an explicit value, never an overloaded null.
- Terminal, broken, absent, and not evaluated are structurally distinct.
- Every lifecycle vocabulary includes terminal-negative outcomes when they can occur.
- Unknown imported values are preserved verbatim as unmapped legacy values.
- Every mapped imported value also retains its original source word.
- Every digest stores its algorithm and canonicalizer identity.
- Every timestamp stores a normalized instant and its source encoding.
- Structural belonging and access scope are separate relations.
- A derived conclusion never destroys the inputs needed to recompute it.
- Clock-derived and cross-table-derived verdicts are computed at read time, not frozen.
- References preserve their original capability, source reference, and origin after resolution.

### Event ledger

Every accepted Command writes its domain change and audit event in one transaction. Events carry a
monotonic sequence, actor, Project, Mission, command identity, timestamp, payload digest, and prior
event digest. The ledger powers Activity, subscriptions, receipts, recovery inspection, and replay
of read projections. It does not replace normalized domain state with pure event sourcing.

### Open Workspace transfer

A full Workspace can be exported as a versioned, openly documented Export Package. It contains
Projects, Missions, Agent identities and roles, policies, memory, Artifacts, Events, Evaluations,
Receipts, Plugin manifests, content digests, and provenance needed to inspect or import the record.
Secret values, live credential handles, transient locks, and machine-specific authority are
excluded. Export is not federation and does not mutate the source Workspace.

Import first validates the package, restores its safe records into a new inactive Workspace, and
makes them inspectable. It then builds a Reconnect Plan that lists every required Resident, Route,
Worker, Connector, Plugin, secret reference, capability, network scope, and budget. Beacon detects
compatible local resources and may suggest an existing secret reference already approved on this
Installation, but suggestions carry no authority.

The operator receives one review showing exact matches, substitutions, missing requirements, and
permission differences. The accepted set can be reconnected together; unresolved items remain
inactive and explain what is missing. The operator does not need to re-enter a credential that can
be safely rebound, but Beacon never copies a secret or activates an imported actor or resource
without confirmation.

### Safety snapshots

Beacon creates an encrypted, integrity-checked Safety Snapshot before installing an update and
before dispatching any Proposal that Policy classifies as high-impact. A snapshot captures the
Workspace record plus declared mutable targets Beacon can safely restore. Snapshot completion is a
precondition: failure blocks the update or Run instead of silently weakening recovery.

Snapshots are recovery points, not exports, authority grants, or promises that external effects can
be undone. They remain visible with their scope, verification result, retention state, and key
reference. Restore is an explicit, attributable Operator action. Update snapshots are created only
after Runs and Worker leases have quiesced.

Default retention keeps the latest ten verified snapshots plus one verified daily snapshot for 30
days, with overlaps stored once. The Operator can change the retention window and storage quota.
Pruning is attributable and never makes an active restore or update lose its required snapshot.

The Operator may add an off-device destination such as removable, network, or independently hosted
storage. Beacon encrypts the snapshot before it leaves managed local storage and verifies the
written copy. No Beacon-hosted service is required. Destination failure is explicit, and Policy
decides whether a verified local snapshot is sufficient for the pending transition.

### Secrets

Provider credentials and signing keys live in the operating system's secret store. SQLite holds
opaque secret references and metadata only. Changing an endpoint invalidates its paired credential.
Secrets never enter Threads, prompts, Artifacts, events, receipts, fixtures, or logs.

### Live-data encryption boundary

Alpha relies on operating-system disk encryption for the live SQLite store and managed Artifact
directory. Beacon does not add transparent application-level encryption to open live Workspace
data or describe that data as Beacon-encrypted. When the platform exposes disk-encryption status,
onboarding and diagnostics report enabled, disabled, or unknown without guessing.
Disabled and unknown states show a persistent but unobtrusive, actionable warning until the
condition changes. The warning opens a one-click, operating-system-specific setup guide and status
recheck. It does not block local Alpha use or change operating-system settings on its own.

Portable Safety Snapshots remain independently encrypted by Beacon before transfer or off-device
storage. Operating-system disk encryption, snapshot encryption, and secret-store protection are
three distinct controls and are never presented as interchangeable.

## 8. Software-factory execution

### Proposal preparation

A software Proposal contains:

- Mission and requesting Agent;
- repository and pinned base revision;
- readable context manifest and digest;
- target path allowlist;
- declared effects;
- provider and Route constraints;
- sandbox and network policy;
- command and tool allowlists;
- required Evaluations and checks;
- time, token, and monetary ceilings;
- rollback strategy;
- canonical policy version;
- digest algorithm and final Proposal digest.

### Sandbox

Alpha 1 executes inside a disposable Git worktree and an operating-system sandbox. Filesystem write
access is limited to declared targets. Network access is denied by default and granted only to named
destinations. Processes receive a clean environment and explicit secret handles. Time, output,
memory, and cost are bounded. The operator can interrupt at any time.

### Receipt

Every Run emits a Receipt containing:

- Proposal and Ruling digests;
- actual base and resulting revisions;
- commands and exit statuses;
- filesystem effects and diff digest;
- model Invocations, tokens, provider, model, and cost;
- Evaluations and checks with exact inputs;
- sandbox and policy versions;
- start, stop, interruption, and failure states;
- residual uncertainty and anything not verified.

A successful process exit is not a successful Mission. Mission success is judged against its
declared conditions and required Evaluations.

## 9. Alpha 1 vertical slice

Alpha 1 demonstrates this exact workflow:

1. The operator signs in locally, then Beacon opens Briefing and discovers local models and Workers.
   The operator edits the proposed Starter Organization, activates selected resources, admits Lead,
   Builder, and Reviewer Residents, selects the bundled disposable Rust task-tracker Sample Project,
   creates its feature brief and Project Charter, grants any cloud Route explicitly, and selects
   Supervised autonomy. A Researcher template remains available.
2. Policy selects a Resident Team Lead, who creates an aligned Mission, defines measurable success
   conditions, and forms a Team.
3. The Team decomposes the Mission, assigns work, and creates a bounded research Specialist.
4. The Team coordinates in Conversations and creates Claims, Evidence, Shared Memory, and Artifacts.
5. The lead produces an Artifact containing the implementation Proposal.
6. Models records every Invocation and enforces the Mission and Agent budgets.
7. Execution prepares a sandboxed candidate change in a disposable worktree.
8. A non-author Agent owns the Evaluation, runs deterministic checks, and applies one rubric
   against pinned inputs.
9. Decisions presents the exact Proposal, evidence, evaluation, risk, diff, and budget to the
   operator.
10. The operator allows or denies it. Denial produces no external mutation.
11. Allow dispatches the prepared Proposal only if its digest and base revision still match.
12. Execution applies the bounded local change and emits a complete Receipt.
13. Briefing shows the Mission outcome, actual spend, Team activity, and unresolved Attention Items.
14. The Specialist closes, promotes required handoff knowledge, and retains attributable Receipts.
15. After restart, the operator can trace every step without relying on a chat transcript.

### Alpha acceptance criteria

- A fresh local install completes the workflow without Athena, Buzz, Minerva, Vulcan, Janus, or
  Icarus running.
- A downloaded Linux AppImage launches on a clean supported host and completes first-run setup.
- An unauthenticated person cannot inspect Workspace data or issue an Operator Command. Successful
  sign-in does not bypass Policy, Gates, Rulings, or execution authority. Sign-in offers a passkey
  first. Password fallback works on the same machine only.
- After a successful local sign-in, loss of network access does not block local Workspaces, Agents,
  models, Workers, or project workflows.
- A Cloud Link remains optional. Alpha synchronizes only theme, keyboard shortcuts, accessibility
  preferences, and notification preferences. It synchronizes no notification contents, events, or
  history; Workspace data; Projects; Missions; Agents; Policy; secrets; credentials; Routes;
  budgets; capabilities; Rulings; or execution authority. The official hosted service is the
  default setup path, while an operator-hosted endpoint uses the same documented protocol and
  boundaries. The local product and self-hosted Cloud Link remain free without a time limit.
  Version history for synchronized preferences remains available locally and when self-hosted;
  official hosting manages its remote copy. Region changes require an explicit migration.
  Self-hosting uses a one-command Docker installer for Linux x86-64 and ARM64. Local use requires
  neither.
- Local account recovery works with an Operator-held offline Recovery Key when Cloud Link is absent
  or unreachable. Setup cannot complete until the Operator confirms that the key was saved. A
  successful rotation immediately invalidates the old key.
- The human session locks after 30 minutes of inactivity by default and honors a changed duration.
  Operating-system screen lock or suspend locks it immediately. Active Agent work continues within
  its existing authority.
- Same-machine MFA may remain disabled. Human remote access is refused unless the exact device was
  approved locally through a QR code and human-compared matching code and MFA is enabled and
  satisfied with a passkey. The device list shows last use and supports one-action revocation.
  Pairings expire after 90 inactive days, and an Attention Item warns seven days before expiry.
  Password fallback cannot authorize remote access. Remote Worker leases remain a separate
  machine-authentication flow.
- Bounded autonomy cannot be enabled in a Workspace for the first time until the Operator
  acknowledges its concise safety notice. The acknowledgement grants no authority and bypasses no
  Gate.
- Every ordinary authenticated launch opens Briefing. Authenticated deep links may open their named
  objects directly.
- Every completed Mission has an Outcome Document linked to its success conditions, delivered
  Artifacts, Evaluations, spend, unfinished work, uncertainty, and Receipts. It exports as canonical
  open Markdown and a regenerated PDF that stays concise. Policy-classified high-impact Missions
  include the verification appendix automatically; the Operator may include it for other Missions.
- Every core operator workflow is keyboard-complete and passes WCAG 2.2 Level AA automated checks
  plus manual testing with Orca, Alpha's reference Linux screen reader.
- The demonstration adds one complete feature to an existing repository and produces an evaluated
  candidate diff plus a final Receipt.
- One operator can supervise at least three independently identified Agents while the Team forms
  itself and assigns its own work.
- Every Agent-created Mission references its Project Charter and records the alignment decision.
- A fresh install proposes Lead, Builder, Reviewer, and Researcher role templates without creating
  Residents or treating Display Names as identities.
- A fresh install includes a bundled, disposable Rust task-tracker Git repository as the optional
  Sample Project without starting a Run, invoking a model, using the network, or touching the
  Operator's real work.
- Background startup remains disabled until the Operator explicitly enables it during onboarding or
  Settings.
- Two Workspaces in one Installation cannot see, spend, execute, or reuse each other's state or
  authority by default.
- Discovered models and Workers remain unusable until the Operator activates them.
- Only the Operator can admit, suspend, or retire a Resident Agent.
- A failed Assignment receives no more than one automatic Recovery Attempt before reassignment with
  a complete Handoff or an Attention Item.
- Team Lead selection records named Track Record evidence, not a composite score, plus its result
  and any operator override.
- A cloud Invocation is refused unless its Project explicitly permits the provider, Route, and
  relevant data scope.
- Every Agent allocation fits within its Mission, Project, and Workspace Budget Envelopes.
- Every Material Artifact names a different author and evaluator Agent.
- Only Attention Items can produce operating-system notifications.
- A full Workspace Export Package uses a documented open format and contains no secret values.
- Import makes safe records inspectable immediately but cannot activate Residents, resources,
  credentials, Plugins, or external authority before the Operator accepts a Reconnect Plan.
- Elapsed time alone cannot pause work unless the Project Charter or Policy names a time gate.
- Every Operator inspection of Private Memory creates an attributable Activity event.
- Every mutation is attributable to an actor, Proposal, policy version, and Receipt.
- Changing a Proposal, base revision, target set, check set, or policy invalidates its Ruling.
- A denied or stale Proposal causes zero repository mutation.
- An interrupted Run reaches a terminal state and explains residual effects.
- Resume Work cannot proceed until the Operator reviews every stopping or unconfirmed Run and
  resolves it or explicitly accepts its uncertainty.
- Pause Work remains operable from the desktop, system tray, and configured global shortcut when
  the main window is closed; every entry point produces the same attributable Command. Beacon
  supplies a default shortcut that the Operator may change or disable.
- Diagnostics report live-data disk encryption as enabled, disabled, or unknown when detectable and
  never describe the open store as Beacon-encrypted. Disabled or unknown status warns without
  blocking local use, remains quietly visible until the condition changes, and opens an
  operating-system-specific setup guide plus status recheck.
- Model usage reconciles by Agent, Mission, Route, provider, and model.
- The attention queue contains only actionable items and names the eligible actor.
- Diagnostic collection defaults off and remains inspectable locally. A scrubbed Alpha feedback
  bundle is previewed and exported manually; Beacon sends nothing automatically.
- State survives restart and the event chain verifies from genesis to head.
- The original source behind every Claim and imported value remains inspectable.
- The desktop and agent interface receive consistent views at the same event cursor.
- The full gate runs from a clean checkout and proves schema, policy, sandbox, and UI contracts.
- Initial Alpha builds are distributed only to invited technical evaluators.

## 10. Delivery plan

### Phase 0 — establish the new trunk

- Proceed with the Alpha architecture under the temporary Beacon title while permanent naming
  continues behind clean application, CLI, and package namespace gates.
- Create the Rust workspace and Tauri shell beside, not inside, the Python prototype.
- Preserve the Python code on a tagged legacy branch as migration-reader research.
- Make this blueprint, glossary, and ADR set the definition of record.
- Add passkey-first local Operator Account sign-in with password fallback, offline continuity, a
  Recovery Key confirmation gate, configurable 30-minute Session Lock, immediate lock on
  operating-system lock or suspend, QR-and-matching-code device pairing, and the remote MFA
  boundary. Keep password fallback same-machine only, invalidate an old Recovery Key immediately
  after rotation, and expose paired-device last use plus one-action revocation. Treat neither
  authentication as execution authority nor Session Lock as Pause Work.
- Add optional Cloud Link synchronization for theme, keyboard shortcuts, accessibility
  preferences, and notification preferences only. Exclude notification contents, work records,
  secrets, and authority. Make the official hosted service the simplest default while preserving
  an operator-hosted endpoint through the same documented protocol. Keep the local product and
  self-hosted Cloud Link free without a time limit. Offer optional official managed sync for $5 per
  month with United States and European Union regions. Preserve synchronized-preference version
  history in local, self-hosted, and managed configurations. Ship one-command Docker self-hosting
  for Linux x86-64 and ARM64, and make every region change an explicit migration.
- Require a short safety acknowledgement before Bounded autonomy is first enabled in a Workspace.
  Keep it educational and separate from authority and Gates.
- Add one command that opens the supervisor, store, and diagnostic event feed.
- Add the editable Starter Organization and first-run admission for Lead, Builder, Reviewer, and
  Researcher role templates.
- Open Briefing after sign-in, offer background startup as an opt-in setting, and keep diagnostic
  collection local and off by default. Export scrubbed Alpha feedback bundles only after Operator
  preview and manual action.
- Add the bundled disposable Rust task-tracker Sample Project without starting work or spending
  resources silently.

Exit: a clean Linux checkout builds, launches, creates a Workspace, and verifies its empty store.

### Phase 1 — trustworthy kernel

- Implement identity, Resident admission, inactive resource discovery, Operator activation,
  capabilities, Projects, Missions, Artifacts, hierarchical Budget Envelopes, and the event ledger.
- Implement multiple Workspace creation, switching, and fail-closed isolation in every interface.
- Implement local-first Route eligibility and explicit Project grants for cloud inference.
- Implement the command, query, and event interfaces plus Tauri, MCP, and CLI adapters.
- Implement policy classification, Autonomy Profiles, and the one mutation path.
- Implement Operator-only Pause Work through the desktop, system tray, and configurable global
  shortcut, with a default the Operator can change or disable, bounded interruption reporting, and
  explicit Resume Work review.
- Implement migrations, schema reconciliation, encrypted Safety Snapshots, and integrity
  verification.
- Report operating-system disk-encryption status without claiming application-level encryption;
  keep a persistent, unobtrusive warning without blocking when status is disabled or unknown, and
  link it to an operating-system-specific setup guide and status recheck.

Exit: two actors cannot exceed their grants, every accepted command is durable and attributable,
and the desktop can recover from a supervisor restart.

### Phase 2 — complete Alpha 1 loop

- Add Conversations, Knowledge, Models, Evaluations, Decisions, and Execution in vertical order.
- Exercise them through one real Mission rather than building seven empty app shells.
- Add sandboxed Git worktrees, independent non-author Evaluation, operator ruling, Receipts, and
  the final Outcome Document with canonical Markdown and a concise regenerated PDF. Include its
  verification appendix automatically for Policy-classified high-impact Missions and optionally
  for other Missions.
- Make the full operator workflow keyboard-complete and verify WCAG 2.2 Level AA with automated
  checks plus manual keyboard and Orca screen-reader testing on supported Linux.
- Package the verified Linux Alpha as an AppImage.

Exit: every Alpha acceptance criterion in section 9 passes in a fresh Installation against the
bundled, disposable Rust task-tracker Sample Project (ADR 0028). The proof requires no Athena,
Buzz, Minerva, Vulcan, Janus, or Icarus runtime, and no fleet repository is an Alpha proving target.

Distribute initial builds as invite-only technical alphas. Expand access only after invited
evaluators complete the core loop and the resulting safety, usability, and installation findings
are triaged.

### Phase 3 — Research and legacy migration

- Add Research as the second production application by composing Conversations, Knowledge, Models,
  Evaluations, and independent report review.
- Prove one question-to-report workflow with canonical open Markdown, structured citations,
  regenerated PDF, source spans, uncertainty, and complete provenance.
- Convert the proven Python adapters into one-shot, read-only migration tools.
- Import six existing systems; ReplayDeck remains a native Beacon capability because no legacy
  implementation exists.
- Preserve source vocabulary, provenance, timestamps, digest algorithms, and unresolved references.
- Require verified-quiescent snapshots for direct database reads.

Exit: Research produces one independently evaluated, provenance-linked report, and a migrated
Project coexists with native Beacon records without silent mapping loss.

### Phase 4 — sustained agent operations

- Add Operations as the third production application after Research proves its workflow.
- Package the Beacon Worker for fleet nodes and add mutually authenticated remote transport.
- Add lease expiry, offline partial Receipts, resource advertisement, scheduling, and reconnection
  reconciliation.
- Add richer autonomous Team formation, reusable Mission templates, resumable schedules, and
  operator-defined bounded automation.
- Add Private and Shared Memory retention, promotion, archival, and access inspection.
- Add the open Workspace Export Package, guided Reconnect Plans, audit and Handoff packages, plus
  cross-machine backup and restore.
- Keep Installations independent; do not treat backup, export, or remote Workers as federation.

Exit: one workstation can supervise long-running Resident and Specialist Agents across remote
Workers without requiring the desktop window to remain open and without weakening Alpha's
authority guarantees.

### Phase 5 — public product and ecosystem

- Publish the source with contribution rules, architectural tests, issue templates, and a stable
  extension SDK, beginning with declarative Agent Packs.
- Ship signed installers with Stable and Preview update channels, backup and recovery tooling, and
  opt-in privacy-preserving diagnostics. Stable is the default; Preview requires opt-in. An update
  may download during work but never installs while any Run or Worker lease is active. Installation
  begins only after its required Safety Snapshot verifies.
- Add a signed native `.deb` package for beta while continuing the AppImage.
- Add sandboxed third-party Plugins, permission review, package signing and checksums, and a
  signed community registry, plus deliberate local sideloading through the same sandbox.
- Expand from Linux to other desktop platforms only after the core authority guarantees pass there.
- Launch the optional official hosted Cloud Link at $5 per month with Operator-selected United
  States or European Union data residency. Make connecting it from an existing local or self-hosted
  setup an in-app flow that requires no server administration. Publish the matching free self-host
  protocol and a one-command Docker installer for Linux x86-64 and ARM64. Keep version history for
  synchronized preferences available in every configuration and provide an explicit
  region-migration flow.

Exit: a new user can install Beacon, complete the operating loop, inspect its safety boundaries,
contribute a change, and install a permissioned extension without trusting undocumented internals.

### Phase 6 — shared ownership

- Add multi-Owner Workspaces only after the solo product is stable and its authority guarantees pass
  in sustained use.
- Use Owner as the only human role. Give every Owner an individual Operator Account and preserve
  individual attribution, least privilege, conflict handling, and complete audit history. Every
  Owner may invite or remove another Owner.

Exit: several Owners can operate one Workspace without shared credentials, ambiguous authority, or
weaker accountability than the solo product.

### Product and funding boundary

The downloadable local product and protocol-compatible self-hosted Cloud Link are complete and free
under Apache-2.0 without a time limit. The optional official hosted Cloud Link costs $5 per month.
It sells the convenience of managed synchronization, storage, and service operation, not features
withheld from local users or self-hosters. Core authority, auditability, recovery, agent operation,
extension capability, preference sync, and preference version history are not removed or degraded
to manufacture an upgrade. A person should want Beacon as a free product before ever considering
the subscription, and moving to managed hosting should require no server expertise.

## 11. Explicit exclusions from Alpha 1

- production deployment;
- public posts, email, or external messaging;
- automatic activation of discovered models or Workers;
- automatic activation of imported Residents, resources, credentials, Plugins, or authority;
- unattended infrastructure mutation;
- live schema migration;
- multi-operator cloud collaboration;
- live synchronization or federation between Installations;
- mobile clients;
- third-party Plugin execution or marketplace;
- compatibility synchronization with the old systems;
- automatic retirement or deletion of any old system;
- arbitrary shell access outside Execution;
- opaque automatic model fallback;
- generic word processing, spreadsheets, or slide authoring.

Beacon is “Office for agents” because it supplies a coherent workplace and shared documents of
work. It is not a clone of human office-document editors.

## 12. Design tests

The architecture remains coherent only while these answers stay true:

- If no Operator Account is authenticated, no Workspace data or Operator Command is accessible.
- If an Operator signs in, authentication alone cannot authorize a Proposal, enable a cloud Route,
  expand a Budget Envelope, or bypass a Gate. Passkey is offered first. Password fallback works
  only on the same machine.
- If the network disappears after local sign-in, every local workflow remains available.
- If a Cloud Link exists, disconnecting it leaves the local Account and Workspace intact. Linking
  alone transfers no Workspace data or authority. Only theme, keyboard shortcuts, accessibility
  preferences, and notification preferences synchronize. Notification contents, events, and
  history do not. The official hosted service and operator-hosted endpoints obey the same protocol
  and data boundaries. The local product and self-hosted Cloud Link remain free without a time
  limit. Official hosting costs $5 per month and requires a United States or European Union region
  choice; self-hosting is available through a one-command Docker installer for Linux x86-64 and
  ARM64. Version history for synchronized preferences remains available locally and when
  self-hosted; official hosting manages its remote copy. Changing regions requires an explicit
  migration.
- If the local Account needs recovery while Cloud Link is unavailable, the offline Recovery Key can
  recover access. First-run setup cannot finish until the Operator confirms that the key was saved.
  A successful rotation makes the old key fail immediately.
- If 30 minutes of human inactivity pass under the default setting, the Session locks without
  pausing active Runs. A configured duration replaces that default. Operating-system screen lock
  or suspend locks the Session immediately.
- If a human connects from an unpaired remote device or without satisfying MFA, Operator access is
  refused. Pairing requires a QR code, a human-compared matching code, and explicit local approval,
  and it remains revocable. The device list shows last use and can revoke a pairing in one action.
  Pairings expire after 90 inactive days, with an Attention Item seven days beforehand. Remote
  access requires a passkey; password fallback cannot satisfy it. A valid Worker lease also
  satisfies neither condition.
- If multi-Owner support would ship before the solo product is stable and its authority model is
  proven, it remains deferred. No Steward, Observer, or shared human account is introduced. Every
  Owner can invite or remove another Owner through an attributable Operator action.
- If Bounded autonomy has never been enabled in a Workspace, it remains unavailable until the
  Operator acknowledges the safety notice. That acknowledgement cannot authorize a Proposal or
  bypass a Gate.
- If Beacon starts normally after authentication, it opens Briefing unless an authenticated deep
  link names a different object.
- If the Operator has not enabled background startup, operating-system login starts no Beacon
  process.
- If the Operator creates an Alpha feedback bundle, Beacon scrubs it and shows its contents before
  manual export. Beacon never sends it automatically.
- If the bundled Sample Project is not explicitly started, it causes no model, network, repository,
  or budget effect.
- If a suite app is deleted, its capability remains usable through the same command interface.
- If the desktop is compromised, it still lacks direct database, filesystem, credential, and
  execution authority.
- If an Agent lies about its role or source kind, guards key on actual capability rather than the
  label it supplied.
- If a type gains a field, a table gains a column, or an enum gains a value, reconciliation fails
  until every layer transports it whole.
- If a model, Worker, check, or tool is discovered but inactive, Beacon cannot invoke or schedule
  it.
- If a model, check, or tool is unavailable, the failure is explicit and no fallback occurs unless
  Policy named it in advance.
- If Project Policy has not granted a cloud provider, Route, and data scope, no cloud Invocation is
  possible.
- If parallel Agents reserve or spend resources, their combined use cannot exceed any shared parent
  Budget Envelope.
- If a Ruling is copied to a changed Proposal, dispatch refuses it.
- If a legacy record maps successfully but lossy, its source vocabulary still survives.
- If an Attention Item has no eligible actor or next action, it is not an Attention Item.
- If a Material Artifact's author also appears as its evaluator, Beacon cannot use that Evaluation
  to authorize effects or claim success.
- If the app reports success, a Receipt names the checks that justify the claim and the checks that
  were not run.
- If a remote Worker is compromised or disconnected, it cannot exceed its lease, grant itself new
  work, or make the control plane report success without a verified Receipt.
- If two Workspaces share one Installation or Worker, neither can query, spend, execute, or reuse
  the other's state or authority without an explicit future cross-Workspace design.
- If selected work moves between Workspaces, only a verified Handoff Package creates new local
  records; it cannot create a live link or transfer secrets, identity, Policy, budget, or authority.
- If a Resource Preset is reused in another Workspace, it creates only inactive suggestions until
  the Operator approves that Workspace's exact resources, permissions, and budgets.
- If another Installation appears, it receives no shared state or authority merely because it can
  reach the same Worker or network.
- If an Export Package contains a secret value or omits required provenance, export fails closed.
- If an imported Workspace has perfect local matches, Beacon may group them into one Reconnect Plan
  but cannot activate the group before the Operator confirms its exact permissions and budgets.
- If any Run or Worker lease is active, update installation cannot begin.
- If an update or high-impact Proposal requires a Safety Snapshot, installation or dispatch cannot
  begin until the snapshot verifies.
- If automatic retention runs, it preserves the latest ten verified snapshots, one daily snapshot
  for 30 days, and every snapshot required by an active restore or update.
- If an off-device destination receives a snapshot, it receives encrypted bytes and the completed
  copy passes integrity verification without relying on a project-hosted cloud service.
- If a target cannot be captured or restored, Beacon identifies that limit and cannot describe the
  effect as reversible merely because a Workspace snapshot exists.
- If the Operator never chooses **Mark caught up**, background refresh and reopening the Briefing
  cannot advance the acknowledged cursor.
- If Light, Dark, or Follow System changes the palette, information hierarchy, contrast, focus, and
  status meaning remain intact.
- If the Operator selects Pause Work, no new Proposal dispatch or Worker lease renewal succeeds;
  active work remains visibly stopping or unconfirmed until Receipt or expiry proves otherwise.
- If the Operator invokes Pause Work from the desktop, tray, or global shortcut, each entry point
  records the same Operator Command even when the main window is closed.
- If the Operator selects Resume Work, every stopping or unconfirmed Run has been reviewed and
  either resolved or explicitly accepted as uncertain.
- If the command palette invokes an action, it crosses the same Command and authority path as the
  equivalent visible control.
- If a Mission is completed, its Outcome Document remains readable without opening Activity or raw
  Trace and links back to the exact Evidence, Evaluations, and Receipts behind it.
- If an Outcome Document exports as PDF, regenerating it from the canonical Markdown cannot create
  a separate or divergent source record. Its verification appendix links the same Evidence,
  Evaluations, and Receipts, and every Policy-classified high-impact Mission includes it.
- If the desktop is used without a pointer, every core workflow remains operable with visible focus
  and correct semantic names.
- If automated accessibility checks pass, Alpha still requires a successful manual workflow test
  with Orca on supported Linux.
- If operating-system encryption status is unavailable, Beacon reports unknown rather than claiming
  live Workspace data is encrypted, keeps a quiet warning visible, offers an operating-system-
  specific setup guide and status recheck, and permits local use to continue.
- If the summary is insufficient, the operator can reach its structured Trace without changing apps.
- If an Agent creates a Mission outside its Project Charter, Beacon refuses it or creates an
  Attention Item for explicit judgment.
- If Policy selects a Team Lead, the result can show the capability, availability, and Track Record
  evidence used.
- If no risk, spend, failure, drift, or explicit time rule fires, elapsed time alone cannot stop
  routine work.
- If an automatic Recovery Attempt fails, Beacon cannot retry again without a reassignment,
  explicit Handoff, or operator decision.
- If an Agent or Plugin tries to install, suspend, or retire a Resident, Beacon refuses it.
- If a Resident creates a Specialist, the Specialist cannot exceed delegated capability, budget,
  time, or network ceilings.
- If Private Memory is promoted, the Shared Memory record identifies its source and the private
  record remains unchanged.
- If a Plugin is compromised, it still lacks raw store access, ambient authority, and an execution
  path outside Policy and Receipts.
- If a Plugin is sideloaded, it receives no broader access or weaker sandbox than a registry Plugin,
  and its source, digest, requested permissions, and warning remain attributable.
- If a registry lists a Plugin, the UI distinguishes its publisher signature from the registry's
  validation attestation and describes neither as proof of safety.

These are product properties, not implementation comments. Tests should exercise them through the
same interfaces used by the desktop and Agents.
