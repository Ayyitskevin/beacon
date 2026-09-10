import axe from "axe-core";
import { vi } from "vitest";

/// The rule set this project's automated accessibility bar is: WCAG 2.1 Level AA.
///
/// Named as tags rather than left to axe's default, which runs every rule it has -- including
/// best-practice and experimental rules that are not WCAG and not what BLUEPRINT's alpha criterion
/// asks for. A bar whose contents change when the dependency ships a new best-practice rule is not
/// a bar; it is a moving target that fails on somebody else's release day.
///
/// `wcag2a` + `wcag2aa` are 2.0 A and AA; `wcag21a` + `wcag21aa` are the 2.1 additions. WCAG 2.1 AA
/// is cumulative over 2.0 AA, so all four tags together are the level, and dropping either pair
/// would silently narrow it.
const WCAG_21_AA: readonly string[] = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"];

/// Rules switched off, each with the reason it is off. A disabled rule is a finding, not a
/// convenience, so this list is short and every entry says what would have to change to re-enable
/// it.
///
/// - `color-contrast`: jsdom has no layout and no rendering, so axe cannot read a computed colour
///   or a background it never painted. The rule does not fail here; it cannot run here. It was
///   already disabled at all sixteen call sites this helper replaces, inline and unexplained. Its
///   coverage is Orca-and-eyes manual testing, which BLUEPRINT's same criterion also requires.
const DISABLED: Readonly<Record<string, { enabled: false }>> = {
  "color-contrast": { enabled: false },
};

/// Run WCAG 2.1 AA over one mounted surface and fail with the rule and the node.
///
/// `toEqual([])` was the assertion before this helper, which reports a violation as a diff of two
/// large objects: the reader gets the whole axe result and has to find the rule in it. A failure
/// here names the rule id, its help text, its WCAG tags and the offending HTML, because the point
/// of the bar is that somebody can act on it.
/// WCAG 2.2 Level AA, which is the criterion `AGENTS.md:117` actually states. Additive on purpose:
/// the 2.1 helper below is the bar every existing surface was measured against, and silently moving
/// it would change sixteen other surfaces' bar in a commit about one new panel. A new surface uses
/// this one; migrating the rest is its own piece of work.
const WCAG_22_AA: readonly string[] = [...WCAG_21_AA, "wcag22a", "wcag22aa"];

export async function expectNoWcag22Violations(container: HTMLElement): Promise<void> {
  return runWcag(container, WCAG_22_AA, "WCAG 2.2 AA");
}

export async function expectNoWcagViolations(container: HTMLElement): Promise<void> {
  return runWcag(container, WCAG_21_AA, "WCAG 2.1 AA");
}

async function runWcag(
  container: HTMLElement,
  tags: readonly string[],
  level: string,
): Promise<void> {
  if (vi.isFakeTimers()) {
    throw new Error("WCAG bar cannot run under fake timers — switch to real timers around the bar");
  }
  const results = await axe.run(container, {
    runOnly: { type: "tag", values: [...tags] },
    rules: DISABLED,
  });
  if (results.violations.length === 0) {
    return;
  }
  const report = results.violations
    .map((violation) => {
      const tags = violation.tags.filter((tag) => tag.startsWith("wcag")).join(" ");
      const nodes = violation.nodes.map((node) => `      ${node.html}`).join("\n");
      return `  ${violation.id} [${violation.impact ?? "unknown"}] (${tags})\n    ${violation.help}\n${nodes}`;
    })
    .join("\n");
  // The 2.1 wording is left exactly as it was: an existing control test holds that literal, and the
  // level is the only part that differs between the two bars.
  throw new Error(`${level}: ${results.violations.length} violation(s)\n${report}`);
}
