# Beacon

Beacon is an office suite for AI agent teams.
Beacon is the current internal working title, not a cleared public brand or software namespace.

One human operator oversees a fleet of agents that can discuss work, plan it, research it,
build it, evaluate it, request authority, execute within bounds, and leave a complete receipt.
The agents run the workplace: they form teams, decompose Missions, assign work, and create bounded
Specialists. The operator checks a concise Briefing, contributes when useful, sets policy and
budgets, and intervenes when judgment or exceptional authority is required.

Beacon proves this solo-operator model first. Multi-Owner Workspaces may follow only after the solo
product is stable and its authority boundaries are proven. Owner is the only planned human role;
each Owner acts through an individually attributable Operator Account.

Beacon is for all agent work: software, research, operations, media, analysis, and work we have not
named yet. It begins with software production, then grows into a broad family of applications over
the same Workspace. Research is the second production application after Software proves the core
loop. Operations follows as the third. The long-term shape is an office suite for Agents, not a
coding-agent wrapper. The goal is a complete, standalone, installable, open-source product.

## The product

Beacon brings the strongest ideas from Athena, Buzz, Minerva, Vulcan, ReplayDeck, Janus, and
Icarus into one product:

- a clear desk for missions, ownership, dependencies, and attention;
- durable conversations between people and agents;
- evidence-backed claims with precise provenance;
- model routing, usage, budgets, and replayable invocation records;
- deterministic and judgment-based evaluation;
- decisions bound to the exact work they authorize;
- bounded execution with observable, verifiable receipts.

These are not seven external products hidden behind one window. They are capabilities of one
suite, sharing one vocabulary, one local store, and one authority model.

## How it feels

Beacon is project-centered. A Project holds the long-lived purpose and working context. Missions
inside it hold finite outcomes. The primary applications are Briefing, Projects, Conversations,
Knowledge, and Software. Model routing, evaluation, authority, execution, activity, and extensions
appear in context instead of crowding the main navigation. Every surface remains a lens over the
same Project and Mission rather than a separate silo.

Every agent has a desk: its assignments, context, allowed tools, budget, messages, and blockers.
A fresh install proposes a professional Starter Organization instead of leaving the operator with
a blank page or silently activating a canned team. The operator reviews every Resident, role,
model binding, permission, and budget before admitting it. The operator has a fleet desk: active
work, health, spend, exceptions, and the small set of items that genuinely require human attention.

Teams combine persistent named agents with durable history and temporary specialists created for
one Mission. Agents keep private working memory and deliberately promote useful knowledge into
shared Project, Mission, or Team memory. One workstation controls the Beacon state and authority
model while remote fleet workers perform bounded work.

One Installation may host several isolated Workspaces, much as an office suite can hold separate
organizations or bodies of work. Workspace data, Agents, policy, budgets, credentials, and
authority never cross that boundary merely because they share an Installation.

Alpha requires the human operator to sign in to a local Operator Account before opening a
Workspace. Sign-in is passkey-first with password fallback. Local workflows remain available
offline after sign-in. Setup does not complete until the Operator confirms that the offline
Recovery Key was saved. Rotating it immediately invalidates the old key. The human session locks
after 30 minutes of inactivity by default, at a configurable duration, and immediately when the
operating system locks or suspends. Already authorized Agent work continues. Multi-factor
authentication is optional for same-machine access, where password fallback is available. Remote
access requires a passkey, local approval through a QR code and matching code, and MFA. Paired
devices show their last use, expire after 90 inactive days, and remain independently revocable with
one action. Beacon creates an Attention Item seven days before expiry.

The Operator may add an optional Cloud Link later. Alpha synchronizes only theme, keyboard
shortcuts, accessibility preferences, and notification preferences. It never synchronizes
notification contents or history, Workspace data, Projects, Missions, Agents, Policy, secrets, or
authority. For broad adoption, the official hosted service is the effortless default. An
operator-hosted endpoint uses the same documented protocol for sovereignty and regulated use. The
complete local product and protocol-compatible self-hosting remain free without a time limit.
Official hosting is an optional $5-per-month convenience for people who want managed sync without
running a server. It lets the Operator choose between United States and European Union data regions
at launch. Version history for synchronized preferences remains available locally and when
self-hosted; the hosted service manages its remote copy. Changing regions is an explicit migration,
never a silent relocation. Self-hosting uses a one-command Docker installer for Linux x86-64 and
ARM64.

The interface is calm, editorial, and quietly academic. Light is selected on first launch. Dark
and Follow System are first-class options that preserve the same hierarchy, contrast, and status
meaning. Full evidence, prompts, model responses, tool activity, policy decisions, and receipts
remain available through deliberate drill-down. Only actionable Attention Items interrupt the
operator. Local resources are preferred after operator activation; cloud inference is an explicit
Project choice. After sign-in, Beacon opens to Briefing.

The desktop is keyboard-complete and targets WCAG 2.2 Level AA. Every completed Mission leaves a
human-readable Outcome Document that exports as open Markdown or a regenerated PDF, so the
operator never has to reconstruct results from logs. PDFs stay concise by default and may include
an optional verification appendix. Policy-classified high-impact Missions include that appendix
automatically.

One global **Pause Work** control stops new dispatches and asks active Runs to interrupt safely.
Beacon reports uncertain or residual effects honestly instead of pretending every machine stopped
at once. The control remains available from the desktop, system tray, and a global shortcut even
when the main window is closed. Beacon supplies a default shortcut that the operator can change or
disable.

## Alpha 1

The first alpha proves one complete software-factory loop:

> idea → plan → research → build → evaluate → approve → execute → receipt

A solo operator creates a Project and states a Mission's outcome and constraints. Resident Agents
form the Team, decompose the work, create Specialists when needed, and produce evidence and a
bounded plan. The operator checks the Briefing, rules on the exact proposed change when required,
and receives a receipt proving what ran and what changed.

Alpha 1 is local-first and Linux-first. Its demonstration adds one complete feature to a bundled,
disposable Rust task-tracker Git repository, from brief through evaluated diff and Receipt. It does
not deploy to production, send public messages, or mutate the retired AI Town systems. Onboarding
offers the repository as an optional Sample Project before the operator connects real work. It also
offers background startup as an opt-in choice; Beacon does not enable it silently. Initial builds
are invite-only technical alphas.

## Promise

Beacon should make capable agent teams feel calm to oversee. Routine work moves without ceremony.
Risky work stops at an honest gate. Nothing disappears silently. Every material claim can show its
evidence, every decision can show what it bound, and every action can show its receipt.

Beacon is intended to be a professional, downloadable open-source application with a documented
contribution path and a permissioned plugin ecosystem. Workspaces remain portable through an open
export. Import restores safe records immediately, then guides the operator through reconnecting
matching local resources and credentials in one explicit review. Its complete local core stays
free, including protocol-compatible self-hosted sync. The optional $5-per-month official service
funds managed cloud sync and operations rather than unlocking product features.

Diagnostics stay on the operator's device and collection is off by default. Alpha feedback uses a
locally generated, scrubbed bundle that the Operator previews and exports manually. Beacon neither
submits the bundle nor sends diagnostics over the network.

Alpha relies on operating-system disk encryption for live Workspace data and says so plainly. It
shows a persistent but unobtrusive warning without blocking local use when encryption is disabled
or cannot be detected. The warning opens a one-click, operating-system-specific setup guide and
status recheck; Beacon does not change system settings on its own.
Portable Safety Snapshots receive Beacon-managed encryption before leaving the Installation.

Beacon is the temporary working title while Alpha architecture begins. Earlier screening rejected
it as a permanent candidate because current AI-agent products already use the name. It does not
authorize a Beacon application ID, CLI command, package namespace, domain, or public release.

The permanent name should be an uncommon real word that evokes a durable knowledge institution
rather than an operations center, Agent, model, assistant persona, temporary workshop, or animal.
It must also be easy to pronounce and spell after hearing it once. Its tone should be professional
with a quiet academic character: thoughtful and enduring, not governmental or corporate. A clean
application, CLI, and package namespace is a hard gate before selection.

Current preliminary collision research is recorded in the
[ten-candidate name screen](docs/research/name-collision-screen-2026-08-27.md) and the follow-up
[Beacon, Recall, and Hivemind screen][three-name-screen]. Orca was proposed, then rejected after
the [Orca collision screen][orca-name-screen] found direct AI-workstation and accessibility-name
conflicts.
No permanent candidate is selected or legally cleared.

[three-name-screen]: docs/research/name-screen-beacon-recall-hivemind-2026-08-27.md
[orca-name-screen]: docs/research/name-screen-orca-2026-08-27.md

The complete product and technical blueprint is in [docs/BLUEPRINT.md](docs/BLUEPRINT.md).
The shared vocabulary is in [CONTEXT.md](CONTEXT.md). Architectural decisions live in
[docs/adr/](docs/adr/). Project stewardship and contribution authority are defined in
[GOVERNANCE.md](GOVERNANCE.md).

## Development foundation

Phase 0 now includes the first Rust supervisor and a native Tauri desktop shell beside the
preserved Python prototype. The supervisor creates one empty local Workspace and returns an
inspectable SQLite diagnostic report:

```sh
cargo run -p mosaicd -- bootstrap
```

The desktop intentionally opens in a locked setup-required state. Until the separately reviewed
Operator Account work exists, its only command returns a closed three-field session status; the UI
receives no Workspace material and exposes no Operator Command.

From `apps/desktop`, use `npm run tauri dev` to launch the development shell. The implementation
checkpoints and verified dependency sources are recorded in
[the Rust foundation](docs/implementation/phase-0-rust-foundation.md) and
[the desktop shell](docs/implementation/phase-0-desktop-shell.md). Run `scripts/gate` to verify the
preserved Python prototype, the Rust workspace, the web bundle, and the native desktop build.

## License

Apache-2.0.
