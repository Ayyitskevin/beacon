import { render } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { expectNoWcagViolations } from "./axe";

/// Every sibling module's source, read by Vite rather than by `node:fs`.
///
/// `import.meta.glob` is the project's own toolchain doing the reading: it needs no `@types/node`,
/// which `tsconfig.app.json` deliberately does not include, and it resolves at build time so a file
/// that stops existing is a build error rather than a silent empty read. The first cut of this test
/// used `node:fs` and passed `npm test` -- vitest does not typecheck -- while failing the desktop
/// BUILD stage on three `TS2591`s. The gate caught it; the suite could not.
const SOURCES: Record<string, string> = import.meta.glob("../*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
});

/// `../Foo.test.tsx` -> `Foo.test.tsx`.
function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/// A test file with its commented-out code removed.
///
/// Measured, not assumed: the first cut of the coverage witness below searched the raw source, and
/// Fable's mutant -- commenting out the Outcome surface's call -- SURVIVED it, because a comment
/// still contains the text. A witness that a comment satisfies is a witness of spelling, not of
/// behaviour.
///
/// Whole-line `//` comments and `/* */` blocks only. Trailing comments after code are left alone
/// deliberately: stripping from the first `//` on a line would also eat the `//` inside a URL in a
/// string literal, and a witness that mangles its own input to be strict is worse than one that is
/// narrow and says so.
function live(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split("\n")
    .filter((line) => !line.trimStart().startsWith("//"))
    .join("\n");
}


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

/// A12 round 2, Fable's second mutant: deleting the Outcome surface's call passed all 161 tests,
/// because nothing witnessed which surfaces are checked. Coverage that is not enumerated is a
/// habit, and a habit is what a rushed commit skips.
///
/// The surface list is DERIVED, never hand-written: a `src/*.test.tsx` file is a surface test when
/// it imports a component from a sibling module that exists (`./Foo` <- `src/Foo.tsx`) and renders
/// it. A hand list would need editing when a surface arrives, which is exactly when nobody edits
/// it -- and the omission would look like a decision rather than a gap.
it("every surface test file runs the WCAG bar", () => {
  const modules = Object.keys(SOURCES).map(baseName);
  const surfaceTests = Object.entries(SOURCES)
    .map(([path, source]) => ({ entry: baseName(path), source: live(source) }))
    .filter(({ entry }) => entry.endsWith(".test.tsx"))
    .map(({ entry, source }) => {
      const rendered = [...source.matchAll(/from "\.\/([A-Z][A-Za-z]*)"/g)]
        .map((match) => match[1])
        .filter((name) => modules.includes(`${name}.tsx`))
        .filter((name) => new RegExp(String.raw`render\(\s*<${name}\b`).test(source));
      return { entry, source, rendered };
    })
    .filter(({ rendered }) => rendered.length > 0);

  // The derivation has to find something, or the assertion below passes over an empty list -- the
  // shape where a coverage test proves nothing because its input silently became empty.
  expect(
    surfaceTests.map(({ entry }) => entry).sort(),
    "the surface derivation found no test files: it has stopped matching",
  ).toEqual([
    "App.test.tsx",
    "AstraC4Lifetimes.test.tsx",
    "AstraC4Recovery.test.tsx",
    "AstraC4Unmount.test.tsx",
    "BriefingReceipts.test.tsx",
    "MissionCompletion.test.tsx",
    "MissionCompletionCompose.test.tsx",
    "MissionOutcomes.test.tsx",
    "ProjectMissions.test.tsx",
    "SampleProject.test.tsx",
  ]);

  for (const { entry, source, rendered } of surfaceTests) {
    expect(
      source.includes("expectNoWcagViolations("),
      `${entry} renders ${rendered.join(", ")} and never runs the WCAG bar`,
    ).toBe(true);
  }
});


it("the WCAG helper refuses fake timers and the next bar still runs", async () => {
  vi.useFakeTimers();
  await expect(expectNoWcagViolations(document.body)).rejects.toThrow("WCAG bar cannot run under fake timers — switch to real timers around the bar");
  vi.useRealTimers();
  await expectNoWcagViolations(document.body);
});

it("every rendered test body has a per-render WCAG witness", () => {
  function bodies(source: string): string[] {
    const found: string[] = []; const headers = [...source.matchAll(/(?:^|\n)\s*(?:it|test)(?:\.each\s*\([\s\S]*?\))?\s*\(/gm)];
    for (const header of headers) { const arrow = source.indexOf("=>", header.index); const open = source.indexOf("{", arrow); if (arrow < 0 || open < 0) continue; let depth = 0; for (let i = open; i < source.length; i += 1) { if (source[i] === "{") depth += 1; else if (source[i] === "}" && --depth === 0) { found.push(source.slice(open + 1, i)); break; } } }
    return found;
  }
  let total = 0; let headers = 0;
  for (const [path, raw] of Object.entries(SOURCES)) { if (!path.endsWith("App.test.tsx")) continue; const source = live(raw); headers += [...source.matchAll(/(?:^|\n)\s*(?:it|test)(?:\.each\s*\([\s\S]*?\))?\s*\(/gm)].length; for (const body of bodies(source)) { total += 1; if (/render\s*\(/.test(body)) expect(body, `${baseName(path)} render lacks WCAG bar`).toContain("expectNoWcagViolations("); } }
  expect(total, `bodies found ${total} / headers ${headers}`).toBe(headers); expect(total).toBeGreaterThanOrEqual(52);
});
