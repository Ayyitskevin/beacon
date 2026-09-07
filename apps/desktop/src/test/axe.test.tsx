import { render } from "@testing-library/react";
import { expect, it } from "vitest";

import { expectNoWcagViolations } from "./axe";


/// A12 round 2, Fable's first mutant: making the helper silent
/// (`if (results.violations.length === 0)` -> `if (true)`) passed all 161 tests, because the
/// broken control that proves the helper can fail lived in a report as a snippet. A control that
/// is not run is not a control, and a bar nothing can make fail is not a bar.
///
/// Three failures, one per success criterion, chosen because they are unambiguous and stable: an
/// image with no accessible name (1.1.1), a form control with no label (4.1.2 via `label`), and a
/// button with no name (4.1.2 via `button-name`). If axe ever renames or retires one of these, this
/// test fails and someone re-reads the rule set on purpose -- which is the right outcome, not a
/// maintenance cost to design around.
it("the helper rejects real WCAG failures, naming each rule", async () => {
  const { container } = render(
    <main>
      {/* eslint-disable-next-line jsx-a11y/alt-text -- deliberately broken: this is the control */}
      <img src="x.png" />
      <input type="text" />
      <button />
    </main>,
  );

  const refusal = await expectNoWcagViolations(container).then(
    () => null,
    (error: unknown) => error as Error,
  );

  expect(refusal, "a silent helper accepted three WCAG failures").not.toBeNull();
  expect(refusal!.message).toContain("WCAG 2.1 AA: 3 violation(s)");
  for (const rule of ["image-alt", "label", "button-name"]) {
    expect(refusal!.message, `the refusal does not name ${rule}`).toContain(rule);
  }
  // The node, not only the rule: a reader has to be able to find the element.
  expect(refusal!.message).toContain("<img");
});
