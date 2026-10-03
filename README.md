# Beacon

**A local-first workspace for AI agent teams, supervised by one human operator.**

Beacon is being built to keep an agent team's projects, decisions, evidence, and execution
receipts in one place. Software development is the first proving workflow; research and
operations are planned to follow.

> **Pre-alpha — for development and evaluation.** Beacon does not yet provide the complete
> autonomous workflow described in its blueprint. There is no stable release. Use disposable
> projects and data. Beacon is the project and local-alpha name; public naming clearance
> remains a separate release decision.

[Current capabilities](#what-works-today) · [Build from source](#build-from-source) ·
[Contribute](CONTRIBUTING.md) · [Architecture](docs/BLUEPRINT.md) · [Security](SECURITY.md)

## Why Beacon

Running several agents creates more than a coding problem: someone must keep track of what
each agent knows, what it may do, what was reviewed, and what actually happened.

Beacon's design brings those responsibilities into one local workspace:

- **Projects and Missions** connect long-term goals to finite pieces of work.
- **Evidence and Evaluations** make claims and reviews inspectable.
- **Proposals and Rulings** bind approval to the exact change being considered.
- **Bounded execution and Receipts** record what ran, what changed, and what remains uncertain.

These are design commitments, not a claim that every workflow is available today.
The [blueprint](docs/BLUEPRINT.md) defines the full product; the
[implementation checkpoints](docs/implementation/) record individual delivered slices.

## What works today

| Area | Implemented scope | Current boundary |
| --- | --- | --- |
| Desktop and local account | Rust supervisor, SQLite store, Tauri/React shell, password enrollment, Recovery Key, and session locking | Passkey sign-in and remote access are planned |
| Projects and teams | Project Charters, Resident lifecycle and configuration, Owner-created Teams and Missions | Autonomous agent coordination is not a completed workflow |
| Local model discovery | Explicit Ollama discovery, inspection, activation, and revocation | Discovery and activation do not invoke a model or start agent work |
| Repository checks | Pinned repository bindings and bounded Declared Check execution on enforcing Linux hosts | An Owner supplies the checks; they do not prove Mission success |
| Change review and dispatch | Backend Proposal, Candidate, Ruling, Evaluation, and dispatch foundations | Dispatch is restricted to Beacon-created disposable copies; no complete desktop workflow |
| Linux packaging | AppImage build and recorded first-run setup checks on Ubuntu 22.04, Debian 12, and Ubuntu 24.04 containers | Container evidence does not establish native desktop or Wayland support |

This is a scoped overview, not a complete inventory. See the
[repository contract](AGENTS.md), [implementation records](docs/implementation/), and
[AppImage first-launch evidence](docs/implementation/phase-1-appimage-first-launch.md).
Some earlier checkpoint descriptions predate later implementation; read each record's scope
and evidence before treating it as a current capability claim.

## The first milestone

Alpha 1 aims to demonstrate one complete software workflow:

**Idea → plan → research → build → evaluate → approve → execute → receipt**

The demonstration uses a bundled, disposable Rust task-tracker repository. The goal is a
feature delivered with independent evaluation, an explicit approval boundary, and a
human-readable outcome. This end-to-end milestone is still in progress.
Initial releases are planned as invite-only technical alphas.

Beyond that milestone, the design expands into research and operations over the same
workspace and authority model. See the [blueprint](docs/BLUEPRINT.md) for the roadmap and
acceptance criteria.

## Build from source

Development currently targets Linux. Start with a disposable checkout and local data.

Prerequisites:

- Git, Rust installed through rustup, and Python 3.
- Node.js **22.23.2** and npm **10.9.8**, as pinned in
  [the desktop package](apps/desktop/package.json).
- The Linux development libraries listed in [CONTRIBUTING.md](CONTRIBUTING.md#setting-up).
- Python **3.12** and a virtual environment if running the full gate, including the legacy tree.

From the repository root, after installing those prerequisites:

```sh
npm --prefix apps/desktop ci
cd apps/desktop
npm run tauri dev
```

The development command builds and stages the supervisor sidecar. First launch opens local
account setup; signing in does not activate agent work.

To inspect the supervisor's bootstrap report separately, from the repository root:

```sh
cargo run -p beacond -- bootstrap
```

For the complete development gate, follow the virtual-environment setup in
[CONTRIBUTING.md](CONTRIBUTING.md), then run:

```sh
scripts/gate all
```

Packaging, checksums, host requirements, and the limits of recorded first-run
testing are covered in the [contributor guide](CONTRIBUTING.md#downloading-the-ci-appimage).

## Architecture at a glance

| Component | Responsibility |
| --- | --- |
| [`crates/beacond`](crates/beacond/) | Rust supervisor, domain logic, managed SQLite persistence, and execution boundaries |
| [`apps/desktop`](apps/desktop/) | Tauri/React desktop interface |
| [`src`](src/) | Preserved Python Legacy Desk and migration knowledge |
| [`docs/adr`](docs/adr/) | Architectural decisions and their rationale |
| [`scripts`](scripts/) | Development gates, packaging, and verification tools |

The supervisor is the only store writer and the authority boundary for external effects.
The desktop is a client of that boundary. The legacy Python desk is historical groundwork;
new product development belongs in the Rust application.

## Contributing

Code, documentation, design feedback, accessibility testing, and reproducible bug reports
are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) for setup and review expectations,
[CONTEXT.md](CONTEXT.md) for terminology, and [GOVERNANCE.md](GOVERNANCE.md) for decision ownership.

For a useful bug report, include the commit, operating system, reproduction steps, expected
behavior, and observed result. Scrub credentials and private workspace data from logs.
Report security concerns through the private process in [SECURITY.md](SECURITY.md).

## Security and project status

Beacon executes repository code and is still under active development. Its
[security policy](SECURITY.md) describes the reporting process and known boundaries,
including Linux sandbox limitations. Live workspace data relies on operating-system disk
encryption.

The intended complete local core remains free. Optional managed services described in the
blueprint are future plans, not services available from this repository.

## License

[Apache License 2.0](LICENSE). Project stewardship is described in
[GOVERNANCE.md](GOVERNANCE.md).
