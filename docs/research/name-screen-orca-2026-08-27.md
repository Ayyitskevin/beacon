# Name collision screen: Orca

Date: 2026-08-27

## Scope and method

This is preliminary naming triage, not legal clearance. The screen tests **Orca** against the brief
for an installable, open-source, local-first office and workstation for autonomous AI-agent teams
under one human operator. The intended name should feel like a professional institution with a
quiet academic character, denote the workplace rather than an Agent, and remain easy to say and
spell.

Research used current first-party product documentation, official repositories, official package
registries, and Verisign's authoritative `.com` RDAP service. No comprehensive trademark,
company-name, common-law, phonetic, international-class, or jurisdiction-specific search was
performed. RDAP confirms registration records only; it does not establish trademark rights or a
domain owner's willingness to sell. **Absence of search results is not clearance.**

No source contradiction affecting the verdict was found. The products below occupy different
subcategories, but the exact-name uses accumulate confusion; two directly occupy Mosaic's initial
AI-agent software wedge.

## Finding

### Meaning and semantic fit

Orca is short, familiar, pronounceable, and easy to spell. The animal suggests intelligence,
coordination, strength, and a social pod, all of which can metaphorically fit agent teams. However,
it sounds more like an Agent persona, mascot, model, or vehicle than a professional institution or
the place where work occurs. Its tone is approachable but lacks the quiet academic character of the
brief.

**Confidence: High.** The pronunciation and metaphor are stable; tone is a product-positioning
judgment rather than a legal fact.

### Exact current software and AI collisions

1. **Orca is already a desktop IDE for multi-agent software work.** Orca's official documentation
   describes a desktop IDE for running Claude Code, Codex, Cursor CLI, and other coding agents side
   by side, with isolated worktrees, diff review, remote SSH, self-hosted servers, scheduling,
   orchestration, memory, and a mobile companion. This nearly duplicates Mosaic's software-first
   workstation wedge and remote-worker direction.
   Source: [Orca product documentation](https://www.onorca.dev/docs)
   — **Very high confidence**, because the first-party description is current and category-direct.

2. **Orca is already the name of a complete AI-agent platform.** Orca Platform documents agent
   development, deployment, user interfaces, storage, databases, CI/CD, safety, tracing,
   observability, and dashboards for teams building scalable agents.
   Source: [Orca Platform documentation](https://docs.orcaplatform.ai/)
   — **High confidence**, because this is another exact first-party platform use in the target
   category.

3. **Orca is already an installable local-first AI coding desktop.** The `@orca_ai/orca` package is
   a self-hosted web IDE for Claude Code with files, Git, terminal, MCP, plugins, a Kanban board,
   local storage, and optional Electron desktop builds. Its global install claims the `orca`
   command.
   Source: [@orca_ai/orca on npm](https://www.npmjs.com/package/@orca_ai/orca)
   — **Very high confidence**, because its local-first IDE and desktop surfaces are unusually
   close to Mosaic's Alpha.

4. **Orca is already a local Rust coding agent.** The `@blade-ai/orca` package describes a Rust,
   cross-platform terminal agent that reads and edits code, runs commands, verifies work, supports
   MCP and plugins, and installs the `orca` command.
   Source: [@blade-ai/orca on npm](https://www.npmjs.com/package/@blade-ai/orca)
   — **High confidence**, because it is an exact, current AI developer-tool and command-name use.

5. **Orca is an established cross-platform desktop application name.** OrcaSlicer is a prominent,
   open-source 3D-printing application distributed for Windows, macOS, and Linux, including through
   Flathub, AppImage, and native installers.
   Source: [official OrcaSlicer repository](https://github.com/OrcaSlicer/OrcaSlicer)
   — **High confidence**, because the maintained first-party repository documents its current
   desktop distribution and exact brand.

6. **Orca is an established enterprise agent-platform brand.** Orca Security markets Orca AI as
   the reasoning layer behind its agents, assistants, workflows, and MCP integrations with Claude,
   Codex, and Cursor. It groups specialized AI teammates into “pods.”
   Source: [Orca Security's Orca AI](https://orca.security/platform/orca-ai/)
   — **High confidence**, because the first-party site shows a mature exact-name software
   platform.

7. **Orca AI is an established autonomous-fleet platform.** The maritime Orca AI product offers
   fleet operations software, an office-facing FleetView, automated lookout capabilities, and
   perception for autonomous vessels. It therefore occupies the same “autonomous fleet under human
   oversight” metaphor even though its industry is shipping rather than software development.
   Source: [Orca AI](https://www.orca-ai.io/)
   — **High confidence**, because the first-party site documents a deployed exact-name AI fleet
   platform.

These are independently significant. The existing multi-agent IDE, complete agent platform, and
local-first coding desktop are direct collisions; the other established uses make the broader
software and autonomous-fleet namespace still more crowded.

### Accessibility conflict

GNOME **Orca** is a free and open-source screen reader that provides speech and braille access to
the graphical desktop through the standard Linux accessibility infrastructure. It has its own user
guide, keyboard commands, preferences, issue tracker, source repository, and established Linux
distribution presence.

Sources: [GNOME Orca](https://orca.gnome.org/) ·
[GNOME Orca user guide](https://gnome.pages.gitlab.gnome.org/orca/help/) ·
[GNOME Orca source](https://gitlab.gnome.org/GNOME/orca)

**Confidence: Very high.** These are the screen reader's official GNOME surfaces.

This is not merely an external collision. Mosaic's [Blueprint](../BLUEPRINT.md) designates Orca as
Alpha's reference Linux screen reader for manual accessibility acceptance testing. Naming the
product Orca would make necessary documentation and support language inherently ambiguous:

- “Test Orca with Orca.”
- “Orca does not announce the Orca toolbar.”
- “Start Orca before launching Orca.”
- “The Orca focus defect reproduces only in Orca.”

Qualifying every reference as “the Orca application” or “GNOME Orca” would reduce some
ambiguity, but it would not fix issue titles, search results, spoken instructions, package
discussions, user reports, or assistive-technology support conversations. The existing npm
products also claim the
`orca` command, further crowding a likely desktop and CLI namespace.

**Accessibility-documentation verdict: Confusing by construction.** A product committed to
keyboard-complete WCAG 2.2 AA behavior should not adopt the exact name of its primary Linux
screen-reader test dependency.

### Domain signal

`orca.com` is registered. Verisign RDAP reports creation on 1993-01-11 and expiration on
2028-01-10.

Source: [Verisign RDAP](https://rdap.verisign.com/com/v1/domain/ORCA.COM)

**Confidence: High** for registration status and dates; no inference is made about trademark rights,
domain use, valuation, or acquisition availability.

## Verdict

**Reject Orca from the final-name pool. Confidence: Very high.**

The decisive reason is not the animal metaphor or the occupied `.com`. An existing Orca already
offers a desktop IDE for orchestrating multiple coding agents, and other exact Orca products occupy
local-first AI coding, agent-platform, security-platform, and desktop-app categories. Reusing the
name would also make Mosaic's own accessibility acceptance documentation confusing because GNOME
Orca is Alpha's approved reference Linux screen reader.

Orca should not join or replace **Colonnade** as the provisional leader. If the team values Orca's
semantic direction, preserve only the themes—coordinated specialists, a capable pod, strength,
memory, and navigation—and seek a name that denotes their workplace without inheriting these exact
software and accessibility collisions.
