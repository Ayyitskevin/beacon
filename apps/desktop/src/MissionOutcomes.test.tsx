import { invoke } from "@tauri-apps/api/core";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { MissionOutcomes } from "./MissionOutcomes";
import { expectNoWcagViolations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const missionId = "22222222-2222-4222-8222-222222222222";
const first = "a".repeat(64);
const second = "b".repeat(64);
const firstEvaluation = "c".repeat(64);
const secondEvaluation = "d".repeat(64);

const publications = [
  {
    publicationId: "44444444-4444-4444-8444-444444444444",
    publicationVersion: 2,
    candidateArtifactDigest: second,
    evaluationDigest: secondEvaluation,
    isCurrent: true,
  },
  {
    publicationId: "55555555-5555-4555-8555-555555555555",
    publicationVersion: 1,
    candidateArtifactDigest: first,
    evaluationDigest: firstEvaluation,
    isCurrent: false,
  },
];

const bytesOf = (text: string): number[] => Array.from(new TextEncoder().encode(text));
const versionOneText = "# Outcome, version one\n\nThe first document.\n";
const versionTwoText = "# Outcome, version two\n\nThe corrected document.\n";

const documentFor = (version: number) => ({
  bytes: bytesOf(version === 1 ? versionOneText : versionTwoText),
  digest: {
    algorithm: "sha256",
    canonicalizer: "beacon.artifact.v1",
    value: version === 1 ? first : second,
  },
  publicationVersion: version,
  publicationVersionCount: 2,
  mediaType: "text/markdown; charset=utf-8",
});

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation((command, payload) => {
    if (command === "mission_publications") return Promise.resolve(publications);
    if (command === "outcome_document") {
      const version = (payload as { version: number | null }).version ?? 2;
      return Promise.resolve(documentFor(version));
    }
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
});

it("passes WCAG 2.1 AA with a document on screen", async () => {
  // A12: this surface had a test file and no accessibility bar. It renders a published Outcome as
  // preformatted text, a version list and a selection control -- the three things most likely to
  // reach a screen reader wrong -- and nothing checked it.
  const { container } = render(<MissionOutcomes missionId={missionId} />);
  expect(await screen.findByRole("heading", { name: "Outcome Documents" })).toBeVisible();
  await expectNoWcagViolations(container);
});

it("lists every version newest first and marks the current one", async () => {
  render(<MissionOutcomes missionId={missionId} />);
  expect(await screen.findByRole("row", { name: /2 \(current\)/ })).toBeTruthy();
  const rows = await screen.findAllByRole("row");
  // Header plus two versions, newest first.
  expect(rows).toHaveLength(3);
  expect(rows[1].textContent).toContain("2 (current)");
  expect(rows[2].textContent).toContain(first);
});

/// The bar: the view shows the STORED bytes for the SELECTED version.
///
/// The mutant this kills asks for the newest version's document every time, which reads as a
/// working view -- the table is right, a document appears, and it is simply the wrong one. Only
/// selecting a superseded version and reading the bytes can tell.
it("shows the stored bytes of the selected version, not the newest", async () => {
  const user = userEvent.setup();
  render(<MissionOutcomes missionId={missionId} />);

  // It opens on the current version.
  const shown = await screen.findByLabelText("Outcome Document");
  expect(shown.textContent).toContain("version two");

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));

  const superseded = await screen.findByLabelText("Outcome Document");
  expect(superseded.textContent).toContain("version one");
  expect(superseded.textContent).not.toContain("version two");
  // And it asked the backend for version 1 by number.
  expect(invokeMock).toHaveBeenCalledWith("outcome_document", { missionId, version: 1 });
});

it("renders the document as text and never as HTML", async () => {
  // One version only, so the fixture is self-consistent: the session guards refuse a history whose
  // versions do not run down to 1, and a document whose version exceeds its count. They caught this
  // fixture when it did not, which is the guards doing their job on the way in.
  invokeMock.mockImplementation((command) => {
    if (command === "mission_publications") {
      return Promise.resolve([{ ...publications[1], isCurrent: true }]);
    }
    if (command === "outcome_document") {
      return Promise.resolve({
        ...documentFor(1),
        bytes: bytesOf("# Heading\n\n<b>not bold</b>\n"),
        publicationVersionCount: 1,
      });
    }
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionOutcomes missionId={missionId} />);
  const shown = await screen.findByLabelText("Outcome Document");
  // The markup arrives as TEXT: the element has no child element, and the tag is visible.
  expect(shown.textContent).toContain("<b>not bold</b>");
  expect(shown.querySelector("b")).toBeNull();
  expect(shown.textContent).toContain("# Heading");
});

it("says so when a Mission has no published Outcome", async () => {
  invokeMock.mockImplementation((command) => {
    if (command === "mission_publications") return Promise.resolve([]);
    return Promise.reject(new Error("unexpected"));
  });
  render(<MissionOutcomes missionId={missionId} />);
  expect(await screen.findByText("This Mission has no published Outcome.")).toBeTruthy();
});

it("clears a failed version's error when another selected version loads", async () => {
  invokeMock.mockImplementation((command, payload) => {
    if (command === "mission_publications") return Promise.resolve(publications);
    if (command === "outcome_document") {
      const version = (payload as { version: number }).version;
      if (version === 2) return Promise.reject({ code: "outcome_unverified" });
      return Promise.resolve(documentFor(version));
    }
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
  const user = userEvent.setup();
  render(<MissionOutcomes missionId={missionId} />);

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Beacon could not verify this Mission's Outcome Document.",
  );
  expect(screen.queryByLabelText("Outcome Document")).not.toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "Show version 1" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("version one");
  expect(screen.queryByRole("alert"), "B8.OUTCOME.SELECTION_ERROR_IS_CURRENT").not.toBeInTheDocument();
  expect(invokeMock).toHaveBeenCalledWith("outcome_document", { missionId, version: 1 });
});

it("labels each Mission's Outcome section with its own heading", async () => {
  invokeMock.mockImplementation((command) => {
    if (command === "mission_publications") return Promise.resolve([]);
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
  render(<>
    <MissionOutcomes missionId={missionId} />
    <MissionOutcomes missionId="33333333-3333-4333-8333-333333333333" />
  </>);
  expect(await screen.findAllByText("This Mission has no published Outcome.")).toHaveLength(2);
  const sections = screen.getAllByRole("region", { name: "Outcome Documents" });
  expect(sections).toHaveLength(2);
  for (const section of sections) {
    const labelId = section.getAttribute("aria-labelledby");
    expect(labelId).toBeTruthy();
    const heading = document.getElementById(labelId!);
    expect(section, "B9.OUTCOME.HEADING_BELONGS_TO_ITS_SECTION").toContainElement(heading);
  }
});

// ---------------------------------------------------------------------------------------------
// B11 -- copying the selected stored Markdown.
//
// The expectation is typed as an INDEPENDENT literal, in a DIFFERENT spelling from the fixture:
// the fixture writes CRLF as \r\n and the accented characters literally, the expectation writes
// the same string with \x0d\x0a and \u escapes. A shared decoder generating both sides would agree
// with itself no matter what the component did.
// ---------------------------------------------------------------------------------------------

const copyOldSource = "# Outcome, version one\r\n\r\nCafé — naïve ✅\ttrailing  \r\n";
const copyOldExpected =
  "# Outcome, version one\x0d\x0a\x0d\x0aCaf\u00e9 \u2014 na\u00efve \u2705\x09trailing  \x0d\x0a";
const copyCurrentSource = "# Outcome, version two\r\n\r\nThe CURRENT document.\r\n";

function mockCopyFixtures() {
  invokeMock.mockImplementation((command, payload) => {
    if (command === "mission_publications") return Promise.resolve(publications);
    if (command === "outcome_document") {
      const version = (payload as { version: number | null }).version ?? 2;
      return Promise.resolve({
        ...documentFor(version),
        bytes: bytesOf(version === 1 ? copyOldSource : copyCurrentSource),
      });
    }
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });
}

function stubClipboard(writeText: unknown): void {
  // MUST be called AFTER `userEvent.setup()`: user-event installs its own `navigator.clipboard`
  // during setup, so a stub installed first is silently replaced and the bar would then be
  // measuring user-event rather than the component.
  Object.defineProperty(globalThis.navigator, "clipboard", {
    configurable: true,
    value: writeText === undefined ? {} : { writeText },
  });
}

// Independently typed: the escapes and spacing are written here, not imported from the component.
const CLIPBOARD_FALLBACK =
  "Beacon could not copy the Markdown.\u0020You can select and copy the displayed document.";

it("copies the exact stored bytes of the selected version, not the current one", async () => {
  mockCopyFixtures();
  const written: string[] = [];
  const user = userEvent.setup();
  stubClipboard((text: string) => {
    written.push(text);
    return Promise.resolve();
  });
  render(<MissionOutcomes missionId={missionId} />);

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("version one");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));

  expect(await screen.findByText("Copied version 1's stored Markdown.")).toBeVisible();
  expect(written, "B11.COPY.EXACT_SELECTED_BYTES").toEqual([copyOldExpected]);
  expect(written[0], "B11.COPY.NOT_THE_CURRENT_VERSION").not.toContain("CURRENT");
});

it("keeps the visible document usable when the clipboard is unavailable", async () => {
  mockCopyFixtures();
  const user = userEvent.setup();
  stubClipboard(undefined);
  render(<MissionOutcomes missionId={missionId} />);

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));
  await screen.findByLabelText("Outcome Document");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));

  expect(await screen.findByText(CLIPBOARD_FALLBACK), "B11.COPY.FALLBACK_IS_USABLE").toBeVisible();
  // The bytes stay on screen, so the fallback the copy names is a real one.
  expect(screen.getByLabelText("Outcome Document")).toHaveTextContent("version one");
});

it("says so with fixed copy when the clipboard rejects", async () => {
  mockCopyFixtures();
  const user = userEvent.setup();
  stubClipboard(() => Promise.reject(new Error("DOMException: write denied by policy")));
  render(<MissionOutcomes missionId={missionId} />);

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));
  await screen.findByLabelText("Outcome Document");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));

  expect(await screen.findByText(CLIPBOARD_FALLBACK), "B11.COPY.NO_RAW_EXCEPTION").toBeVisible();
  expect(
    screen.queryByText(/DOMException|denied by policy/),
    "B11.COPY.NO_RAW_EXCEPTION",
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(/Copied/),
    "B11.COPY.SUCCESS_ONLY_AFTER_RESOLVE",
  ).not.toBeInTheDocument();
});

it("does not leave a copy result beside a different version after a late clipboard settlement", async () => {
  mockCopyFixtures();
  // No initialiser: `= null` narrows the declared type to `null`, because TypeScript cannot order
  // an assignment made inside the Promise executor, and the later call then fails to type-check.
  let settle: (() => void) | undefined;
  let writeStarted = false;
  const user = userEvent.setup();
  stubClipboard(
    () =>
      new Promise<void>((resolve) => {
        writeStarted = true;
        settle = resolve;
      }),
  );
  render(<MissionOutcomes missionId={missionId} />);

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));
  await screen.findByLabelText("Outcome Document");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));

  // The operator moves to another version while the clipboard write is still pending.
  await user.click(screen.getByRole("button", { name: "Show version 2" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("CURRENT");

  // Harness check: the component really did start a clipboard write that is still pending, so the
  // bar below is about a late settlement rather than about a write that never happened.
  expect(writeStarted, "B11.COPY.LATE_SETTLEMENT_HARNESS").toBe(true);
  settle?.();
  await new Promise((resolve) => setTimeout(resolve, 0));

  expect(
    screen.queryByText(/Copied version/),
    "B11.COPY.LATE_SETTLEMENT_IS_DISCARDED",
  ).not.toBeInTheDocument();
});


it("keeps a new copy pending when an older copy settles after returning to the same version", async () => {
  mockCopyFixtures();
  const settlements: Array<() => void> = [];
  const user = userEvent.setup();
  stubClipboard(() => new Promise<void>((resolve) => settlements.push(resolve)));
  render(<MissionOutcomes missionId={missionId} />);

  await user.click(await screen.findByRole("button", { name: "Show version 1" }));
  await screen.findByLabelText("Outcome Document");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));
  expect(settlements, "B11.COPY.FIRST_REQUEST_STARTED").toHaveLength(1);

  await user.click(screen.getByRole("button", { name: "Show version 2" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("CURRENT");
  await user.click(screen.getByRole("button", { name: "Show version 1" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent("version one");
  await user.click(screen.getByRole("button", { name: "Copy Markdown" }));
  expect(settlements, "B11.COPY.SECOND_REQUEST_STARTED").toHaveLength(2);
  expect(screen.getByRole("button", { name: "Copy Markdown" })).toBeDisabled();

  try {
    await act(async () => settlements[0]());
    expect(
      screen.getByRole("button", { name: "Copy Markdown" }),
      "B11.COPY.OLDER_REQUEST_CANNOT_RELEASE_NEW_REQUEST",
    ).toBeDisabled();
    expect(screen.queryByText(/Copied version/), "B11.COPY.NEW_REQUEST_HAS_NOT_RESOLVED")
      .not.toBeInTheDocument();
  } finally {
    await act(async () => settlements[1]());
  }
  expect(await screen.findByText("Copied version 1's stored Markdown.")).toBeVisible();
});

const soleReadPublication = [
  {
    publicationId: "66666666-6666-4666-8666-666666666666",
    publicationVersion: 1,
    candidateArtifactDigest: first,
    evaluationDigest: firstEvaluation,
    isCurrent: true,
  },
];

/// B14: the sole published version's read fails, and the operator can ask for it again.
///
/// Before this, the document effect keyed on [missionId, selected]; with one publication already
/// selected, clicking its own button changed nothing and there was no way back short of restarting
/// the desktop.
///
/// The second read is held UNRESOLVED on purpose. Resolving it immediately -- which this bar did
/// first -- never lets the pending state exist, so suppressing the loading line on retry alone kept
/// every Outcome bar green. The payload is captured and asserted in the test body, never inside the
/// mock: an assertion that throws in there rejects the promise, and the component catches it as a
/// read failure, so the bar would report the wrong thing.
it("retries the selected version's failed document read without restarting", async () => {
  const user = userEvent.setup();
  let documentCalls = 0;
  const payloads: unknown[] = [];
  let resolveSecond: ((value: unknown) => void) | null = null;
  invokeMock.mockImplementation((command, payload) => {
    if (command === "mission_publications") return Promise.resolve(soleReadPublication);
    if (command === "outcome_document") {
      documentCalls += 1;
      payloads.push(payload);
      if (documentCalls === 1) return Promise.reject(new Error("the read failed"));
      return new Promise((resolve) => {
        resolveSecond = resolve;
      });
    }
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });

  render(<MissionOutcomes missionId={missionId} />);
  expect(await screen.findByRole("alert"), "B14.RETRY.FAILURE_IS_SHOWN").toBeInTheDocument();
  // The harness must have made a real call, or a retry could "pass" against nothing.
  expect(documentCalls, "B14.RETRY.FIRST_READ_HAPPENED").toBe(1);

  await user.click(screen.getByRole("button", { name: /retry/i }));

  // The retry really asked, and asked for the retained Mission and version.
  expect(documentCalls, "B14.RETRY.SECOND_READ_HAPPENED").toBe(2);
  expect(payloads[1], "B14.RETRY.EXACT_TUPLE").toMatchObject({ missionId, version: 1 });

  // While that second read is still in flight, the operator can see that it is.
  expect(
    screen.getByText(/loading version 1/i),
    "B14.RETRY.PENDING_IS_VISIBLE",
  ).toBeInTheDocument();
  expect(screen.queryByRole("alert"), "B14.RETRY.ERROR_CLEARS").not.toBeInTheDocument();

  await act(async () => {
    resolveSecond?.(documentFor(1));
    await Promise.resolve();
  });

  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent(
    "Outcome, version one",
  );
  expect(screen.queryByRole("alert"), "B14.RETRY.ERROR_STAYS_CLEARED").not.toBeInTheDocument();
  expect(
    screen.queryByText(/loading version/i),
    "B14.RETRY.LOADING_DISAPPEARS",
  ).not.toBeInTheDocument();
});

/// B14: a read that settles late cannot speak for the selection that replaced it.
///
/// Retry only appears once a read has FAILED, so an unsettled first attempt cannot coexist with a
/// retry click for a sole publication -- my first version of this bar tried that and was wrong
/// about the UI. The reachable deferral is a new selection while a read is still in flight, which
/// is the same guard the retry path relies on.
it("ignores a read that settles after the operator selected another version", async () => {
  const user = userEvent.setup();
  let rejectVersionTwo: ((cause: unknown) => void) | null = null;
  let documentCalls = 0;
  invokeMock.mockImplementation((command, payload) => {
    if (command === "mission_publications") return Promise.resolve(publications);
    if (command === "outcome_document") {
      documentCalls += 1;
      const version = (payload as { version: number | null }).version ?? 2;
      if (version === 2) {
        return new Promise((_resolve, reject) => {
          rejectVersionTwo = reject;
        });
      }
      return Promise.resolve(documentFor(version));
    }
    return Promise.reject(new Error(`unexpected command: ${String(command)}`));
  });

  render(<MissionOutcomes missionId={missionId} />);
  // Version 2 is genuinely in flight before anything replaces it.
  expect(await screen.findByText(/loading version 2/i)).toBeInTheDocument();
  expect(documentCalls, "B14.DEFERRED.FIRST_READ_PENDING").toBe(1);

  await user.click(screen.getByRole("button", { name: "Show version 1" }));
  expect(await screen.findByLabelText("Outcome Document")).toHaveTextContent(
    "Outcome, version one",
  );

  await act(async () => {
    rejectVersionTwo?.(new Error("the older read failed"));
    await Promise.resolve();
  });

  expect(
    screen.queryByRole("alert"),
    "B14.DEFERRED.OLDER_READ_CANNOT_REPLACE_NEWER",
  ).not.toBeInTheDocument();
  expect(screen.getByLabelText("Outcome Document")).toHaveTextContent("Outcome, version one");
});
