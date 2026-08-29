# Beacon

Beacon is an office suite where AI agent teams perform project work under one human operator.
This glossary defines the product language shared by every capability and interface.
Beacon is the canonical project and local-alpha name. Public distribution still requires its
separate naming and release gates.

## Organization

**Operator**:
The human who owns a Workspace, sets its policy, and holds final authority over gated actions.
_Avoid_: Admin, user, approver

**Owner**:
The sole human role in a future multi-operator Workspace. Each Owner acts as an individually
attributable Operator rather than through a shared identity.
_Avoid_: Steward, Observer, Admin

**Operator Account**:
The local authenticated human identity required to enter an Installation and act as its Operator.
_Avoid_: Workspace, tenant, subscription, model-provider account

**Cloud Link**:
An optional association that synchronizes an explicit set of non-secret interface preferences for
a local Operator Account without carrying work records, notification contents, secrets, or
authority.
_Avoid_: Cloud Workspace, synchronization, remote authority

**Linked Device**:
A device participating in Cloud Link preference synchronization without receiving remote access,
Worker identity, or authority.
_Avoid_: Paired Device, Worker, trusted device

**Recovery Key**:
An Operator-held offline credential whose safekeeping is required to recover access to a local
Operator Account.
_Avoid_: Password, Cloud Link, Workspace key

**Session Lock**:
A human-access boundary that hides Workspace data and blocks Operator Commands while authorized
Agent work continues.
_Avoid_: Pause Work, sign-out, shutdown

**Paired Device**:
A remote human-access device explicitly verified and approved from the local Installation and
independently revocable.
_Avoid_: Worker, Cloud Link, trusted network

**Agent**:
A non-human actor with an identity, role, capabilities, budget, and accountable activity history.
_Avoid_: Bot, worker, model

**Agent Role**:
A professional functional responsibility, such as Lead, Researcher, or Builder, that remains
visible independently of an Agent's optional Display Name.
_Avoid_: Persona, title, name

**Display Name**:
An optional human-friendly name for an Agent that never serves as identity or authority.
_Avoid_: Agent ID, role, persona

**Resident Agent**:
An Operator-admitted persistent Agent whose identity, role, history, and Track Record continue
across Missions. Its lifecycle is inactive, active, suspended, or retired. Lifecycle controls
availability for later assignment and never grants a model, Capability, budget, or execution
authority; retirement is terminal while identity and history remain auditable.
_Avoid_: Permanent agent, primary agent

**Resident Configuration**:
An append-only Operator decision that binds one Resident to one exact active Resource Activation,
an explicit Beacon Capability set, and the Alpha no-spend boundary for Team eligibility. It is
separate from lifecycle and never creates a Route, Mission, Run, external effect, or execution
authority. Clearing it or revoking its exact Resource Activation makes the Resident ineligible for
new Team formation while preserving history.
_Avoid_: Agent profile, role preset, Route, execution grant

**Specialist Agent**:
A temporary Agent created for one bounded purpose, with Mission-scoped capabilities and budget.
_Avoid_: Subagent, disposable agent, helper

**Worker**:
A local or remote machine runtime that carries out leased Runs without owning authority or truth.
_Avoid_: Agent, node, executor

**Discovered Resource**:
A local model or Worker Beacon can identify but cannot schedule or invoke until the Operator
activates it.
_Avoid_: Available resource, eligible Route, installed Agent

**Resource Preset**:
A reusable, non-secret suggestion of resource requirements and settings that carries no activation
or authority into a Workspace.
_Avoid_: Shared grant, credential bundle, active configuration

**Team**:
A durable named group of one or more distinct Agent identities assembled inside one Project. Its
formation preserves the exact Resident Configuration used for each member, while current
eligibility is derived without rewriting that evidence. Membership aggregates no Capability or
authority, and a Team has no permanent lead.
_Avoid_: Swarm, pool

**Team Lead**:
The Resident Agent accountable for coordinating one Mission, selected under Policy from eligible
Agents using capability fit, availability, and attributable Track Record.
_Avoid_: Manager, supervisor, orchestrator

**Starter Organization**:
An editable onboarding proposal of Resident roles, Policies, and resource mappings that creates no
Agent or authority until the Operator accepts it.
_Avoid_: Default team, canned crew, preinstalled Agents

**Workspace**:
An isolated local trust and authority domain containing Projects, Agents, Policy, credentials,
budgets, memory, and activity.
_Avoid_: Tenant, account, installation

**Installation**:
One independent Beacon control plane and managed store hosting one or more isolated Workspaces and
their leased Workers.
_Avoid_: Worker, fleet, federation

**Project**:
A long-lived body of related purpose, context, artifacts, and Missions.
_Avoid_: Workspace, repository, initiative

**Sample Project**:
An optional disposable onboarding Project for learning Beacon's complete work loop without
touching the Operator's real work.
_Avoid_: Demo mode, tutorial Workspace, mandatory example

**Project Charter**:
The Operator-authored purpose, desired outcomes, constraints, prohibitions, and authority envelope
that determine which self-created Missions belong in a Project.
_Avoid_: Prompt, brief, Mission description

**Charter Revision**:
An immutable Operator-authored version of a Project Charter to which Missions and alignment
decisions bind exactly.
_Avoid_: Current prompt, live brief, mutable instructions

**Mission**:
A finite desired outcome owned by one Team, with explicit success conditions, constraints,
prohibitions, and lifecycle. Admission binds one exact Charter Revision and Team-member
configuration snapshot without selecting a Team Lead or granting execution authority.
_Avoid_: Task, ticket, job

**Assignment**:
The accountable relationship between an Agent and work within a Mission.
_Avoid_: Lease, ownership

**Handoff**:
A durable transfer packet that gives a new Agent the Assignment state, relevant context, Artifacts,
open risks, and prior verification without changing authorship.
_Avoid_: Summary, message, prompt

**Handoff Package**:
An immutable, provenance-preserving selection of records offered by one Workspace for explicit
adoption by another without transferring secrets, identity, or authority.
_Avoid_: Live link, shared Workspace, synchronization

**Recovery Attempt**:
One bounded continuation of failed or interrupted work under unchanged authority and remaining
budget before Beacon requires reassignment or intervention.
_Avoid_: Retry loop, rerun, restart

**Track Record**:
An Agent's evidence-linked history of outcomes, Evaluations, policy compliance, spend, and Receipts
used for future selection without collapsing into one score or becoming ambient authority.
_Avoid_: Score, rank, reputation

**Budget Envelope**:
A hard ceiling on money, tokens, time, or another measured resource, delegated from a broader scope
to a Workspace, Project, Mission, or Agent.
_Avoid_: Balance, credit, estimate

## Knowledge and communication

**Thread**:
An ordered conversation attached to a Project, Mission, Artifact, Claim, or Run.
_Avoid_: Chat, channel

**Claim**:
A statement that can be supported, contradicted, superseded, or left uncertain.
_Avoid_: Fact, note, assertion

**Evidence**:
A precisely identified source used to evaluate a Claim.
_Avoid_: Context, citation, proof

**Finding**:
The recorded result of evaluating a Claim against Evidence, including uncertainty.
_Avoid_: Insight, answer, conclusion

**Artifact**:
A versioned work product with content identity and provenance, such as a plan, source tree, diff,
document, dataset, evaluation fixture, or receipt.
_Avoid_: File, attachment, output

**Material Artifact**:
An Artifact used to support a Ruling, authorize effects, claim Mission success, or become a durable
shared deliverable.
_Avoid_: Important file, final output, approved Artifact

**Outcome Document**:
The human-readable Artifact that closes a completed Mission by summarizing its result, evidence,
spend, unfinished work, and linked Receipts.
_Avoid_: Status update, Receipt, activity summary

**Export Package**:
A complete, versioned, openly documented Workspace transfer that preserves provenance and excludes
secret values.
_Avoid_: Backup, database copy, proprietary archive

**Reconnect Plan**:
An Operator-reviewable mapping from an imported Workspace's required Routes, Workers, Plugins, and
Connectors to eligible resources on the current Installation.
_Avoid_: Automatic restore, credential migration, silent activation

**Private Memory**:
Agent-scoped working knowledge visible to its owning Agent and to the Operator through deliberate
inspection, but not automatically visible to other Agents.
_Avoid_: Hidden memory, scratchpad, chain of thought

**Shared Memory**:
Durable knowledge intentionally available within a named Workspace, Project, Mission, or Team,
with its source and promotion history preserved.
_Avoid_: Global memory, context dump, common prompt

**Memory Promotion**:
The explicit creation of a Shared Memory record from Private Memory or another source without
altering or erasing the source record.
_Avoid_: Sync, publish, copy

## Models and evaluation

**Provider Feature**:
A model provider's advertised operation, such as completion, embedding, vision, tools, or thinking,
used as evidence during resource review but never as a Beacon Capability or grant of authority.
_Avoid_: Capability, permission, model power

**Resource Activation**:
An Operator's revocable Workspace-scoped decision that an exact Discovered Resource and approved
Provider Feature subset may be considered for later Route binding.
_Avoid_: Enable model, Capability grant, Route

**Invocation**:
One recorded request to a model and its response, provider, model identity, inputs, usage, cost,
and timing.
_Avoid_: Prompt, call, completion

**Route**:
A policy that selects an eligible model for a purpose under capability, privacy, and budget bounds.
_Avoid_: Model alias, fallback

**Evaluation**:
A repeatable procedure that judges an Artifact or Run against declared criteria.
_Avoid_: Test, review, check

**Replay**:
A new Run of an Evaluation against pinned inputs and a recorded environment.
_Avoid_: Retry, rerun

## Authority and execution

**Policy**:
Operator-authored rules that classify and bound actions before execution.
_Avoid_: Settings, guardrail, prompt

**Project Policy Revision**:
An immutable Operator-authored version of Project rules, bound to one exact Charter Revision and
used to evaluate future decisions without granting authority itself.
_Avoid_: Live settings, appointment, permission grant

**Autonomy Profile**:
The Project-level posture that determines which classes of actions may proceed automatically.
_Avoid_: Mode, trust level

**Proposal**:
An immutable description of intended effects, inputs, bounds, checks, and budget, identified by its
digest.
_Avoid_: Plan, request, action

**Gate**:
A point where a Proposal cannot proceed until an eligible decider records a Ruling.
_Avoid_: Approval, prompt, confirmation

**Ruling**:
An allow or deny decision bound to one exact Proposal digest and one policy version.
_Avoid_: Approval, permission, vote

**Run**:
One attempt to carry out a Proposal or Evaluation in a named environment.
_Avoid_: Job, execution, session

**Receipt**:
An immutable account of a Run's inputs, effects, checks, usage, result, and content identities.
_Avoid_: Log, result, report

**Attention Item**:
An unresolved condition for which the Operator or a named Agent can take a concrete next action.
Only this object may request an operating-system notification.
_Avoid_: Notification, alert, inbox item

**Pause Work**:
An Operator-controlled Installation state that prevents new work from starting and asks active
Runs to reach an honest terminal state before work resumes.
_Avoid_: Kill switch, shutdown, cancel all

**Resume Work**:
The Operator's decision to end Pause Work after reviewing every stopping or unconfirmed Run and
its known residual effects.
_Avoid_: Unpause, automatic restart, reconnect

**Briefing**:
The Operator's compact, evidence-linked view of outcomes, risk, spend, health, and the few matters
that need contribution or authority.
_Avoid_: Dashboard, activity feed, status wall

## Integrity

**Safety Snapshot**:
An encrypted, integrity-checked recovery point for a Workspace and the declared mutable targets
Beacon can restore before a risky transition.
_Avoid_: Export Package, rollback guarantee, backup promise

**Trace**:
The ordered low-level record of prompts, responses, tool activity, decisions, and outputs behind a
summary or Receipt.
_Avoid_: Chain of thought, transcript, log

**Provenance**:
The identity and history needed to answer how a record or Artifact came to exist.
_Avoid_: Metadata, source

**Binding**:
The exact object, intent, handle, step, or bytes to which a Ruling applies.
_Avoid_: Link, reference, scope

**Drift**:
A measured mismatch between a Binding and the object currently presented in its place.
_Avoid_: Change, staleness, difference

**Capability**:
A named class of operations an actor is allowed to request through Beacon's authority interface.
_Avoid_: Permission, role, tool

**Plugin**:
A versioned extension package that declares its capabilities, permissions, settings, and compatible
Beacon interfaces and runs without direct access to Beacon's store or authority internals.
_Avoid_: Add-on script, database extension, in-process module

**Agent Pack**:
A Plugin that contributes versioned Agent roles, instructions, tools, evaluation criteria, and
recommended Policies without granting those Agents authority.
_Avoid_: Persona, prompt bundle, autonomous app
