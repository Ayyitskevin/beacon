# Beacon discovery drafts — 2026-09-29

These are source-grounded documentary drafts for review, not accepted interfaces,
implementation instructions, runtime verification, or permission to change authority.
Source base: `84fc76a662c57ccad9aa06c8b59bbdf8b41e1ec4` (cached local main).
The prior W03 map used a different retained candidate; its facts are not silently
transferred to this base.

| Draft | New documentary result | Source identities |
|---|---|---|
| [AC22: Assignment recovery, Handoff and Attention](assignment-recovery-boundary.md) | Separates Run reconstruction/terminal evidence from Assignment recovery lineage, one-attempt enforcement and reassignment; names reusable evidence and missing witnesses. | [AC22 pins](assignment-recovery-sources.json) |
| [AC27: Attention to OS notifications](attention-notification-boundary.md) | Separates bounded Receipt reads and quiet in-app status from an actionable Item, an OS request, and observed platform delivery. | [AC27 pins](attention-notification-sources.json) |

Source-pin JSON uses `repo` for paths relative to this repository at the base SHA;
`workspace` denotes retained fleet evidence under mickey's Codex workspace. Those
workspace records are not shipped here, so reproducing their hashes needs the
filed evidence package on mickey. Repository links and numbered source lines resolve
against the inspected base. Source tests were read, not run or credited as runtime
acceptance. The report package is
`/home/kevin-lee/ai-workspace/codex/reports/beacon-discovery-afternoon-20260929/`.

The next frontier retains existing M6 actor/persistence and budget decisions,
Attention ownership/lifecycle and acceptance evidence gaps. The drafts choose no
new schema, actor grammar, permission, Tier, retry policy or notification policy.
They create no owner answer, provider request, hosted CI run, runtime activation or
landing permission. CI resource recovery and all review/merge-lane decisions are
separate parked work. No existing draft PR is altered by these documents.
