import type { ExecutionSummary, PreflightOutcome } from "./session";

/** What each outcome means to the operator. Preflight never runs a check, so
 *  none of these is a check result.
 *
 *  One definition, imported by every surface that shows an outcome. Two copies
 *  agreeing today is not the same as one vocabulary: TypeScript forces both to
 *  cover the enum and cannot keep their words equal, which is exactly how a
 *  Briefing row and a Project panel start describing one recorded fact
 *  differently.
 *
 *  `title` and `detail` are the Project panel's two lines; `summary` is the one
 *  line a compact Briefing row has space for. Three fields, one definition --
 *  so a surface picks which it needs and none of them invents words.
 */
export const preflightOutcomeCopy: Record<
  PreflightOutcome,
  { title: string; detail: string; summary: string }
> = {
  verified: {
    summary: "Execution would be admissible — not a passing check",
    title: "Execution would be admissible",
    detail: "Every step held and a sandbox is available. This is not a passing check.",
  },
  sandbox_unavailable: {
    summary: "Refused — no sandbox",
    title: "Refused — no sandbox",
    detail:
      "Every verification step held. Execution is refused because Beacon has no sandbox capability yet.",
  },
  source_kind_unsupported: {
    summary: "Refused — Git URL",
    title: "Refused — Git URL",
    detail:
      "Confirming a pinned revision in a remote needs a network fetch the operating envelope denies. Bind a local path to preflight it.",
  },
  binding_stale_for_charter: {
    summary: "Refused — binding is stale",
    title: "Refused — binding is stale",
    detail: "The Charter was republished after this binding. Rebind before preflighting.",
  },
  repository_unreadable: {
    summary: "Refused — repository unreadable",
    title: "Refused — repository unreadable",
    detail: "The declared source is missing, unreadable, or not a Git repository.",
  },
  base_revision_unreachable: {
    summary: "Refused — base revision absent",
    title: "Refused — base revision absent",
    detail: "The pinned commit is not present in that repository's object store.",
  },
  declared_checks_changed: {
    summary: "Refused — the binding moved",
    title: "Refused — the binding moved",
    detail: "The Repository Binding changed while this preflight was inspecting it.",
  },
  working_directory_escapes: {
    summary: "Refused — working directory escapes",
    title: "Refused — working directory escapes",
    detail: "A check's resolved working directory lies outside the repository root.",
  },
  preflight_failed: {
    summary: "Refused — a step errored",
    title: "Refused — a step errored",
    detail: "A verification step could not complete. This is not the same as a negative answer.",
  },
  preflight_timed_out: {
    summary: "Refused — a step did not finish",
    title: "Refused — a step did not finish",
    detail: "A verification step ran out of budget, so its answer is unknown rather than negative.",
  },
};

/** What each execution summary means to the operator.
 *
 *  Same rule as the preflight map above: one definition, so a Briefing row and
 *  any later surface describe one recorded fact with one vocabulary. None of
 *  these says a check PASSED -- ADR 0031 is explicit that `completed` records
 *  that every Declared Check ran, and nothing about what it concluded.
 *
 *  `not_admissible` is written from the landed pin-at-open meaning: the
 *  execution Run opens first and pins its Repository, Charter and policy
 *  revisions, and the embedded preflight reads those PINNED revisions. It is
 *  not a statement about the Project's current binding.
 */
export const executionSummaryCopy: Record<ExecutionSummary, { summary: string }> = {
  not_admissible: {
    summary: "refused: the preflight that opened this Run did not admit it",
  },
  worktree_unavailable: {
    summary: "refused: no disposable worktree could be created",
  },
  completed: {
    summary: "every Declared Check ran — this does not say they passed",
  },
  incomplete: {
    summary: "dispatched, but at least one check has no established outcome",
  },
};
