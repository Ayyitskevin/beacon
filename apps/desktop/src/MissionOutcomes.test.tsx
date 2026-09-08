import { invoke } from "@tauri-apps/api/core";
import { render, screen } from "@testing-library/react";
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
