import { invoke } from "@tauri-apps/api/core";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";

import { MissionOutcomePreparation } from "./MissionOutcomePreparation";
import { expectNoWcag22Violations } from "./test/axe";

// This is a joined frontend witness, not a packaged or native-store journey.
// Keep the parent, document child and session validation real; only the native
// process boundary is replaced. No account, model or provider is contacted.
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const invokeMock = vi.mocked(invoke);

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
});

it("M2.UI.KEYBOARD_PREPARE_THEN_VIEW: normal traversal reaches both explicit actions and the stored document", async () => {
  const missionId = "99999999-9999-4999-8999-9999999c7000";
  const manifestDigest = "a".repeat(64);
  const candidate = {
    missionId,
    manifestId: "66666666-6666-4666-8666-666666666666",
    manifestDigest: {
      algorithm: "sha256",
      canonicalizer: "beacon.outcome.manifest.v2",
      value: manifestDigest,
    },
    candidateArtifactDigest: {
      algorithm: "sha256",
      canonicalizer: "beacon.artifact.v1",
      value: "b".repeat(64),
    },
    candidateContentDigest: {
      algorithm: "sha256",
      canonicalizer: "beacon.raw.v1",
      value: "da89301a7322f1479276538550cc162a1568c668f5f15b3ff257fdc57595ca62",
    },
    candidateContentBytes: 26,
    mediaType: "text/markdown; charset=utf-8",
  };
  invokeMock
    .mockResolvedValueOnce({ ...candidate, appendixRequest: "include", appendixMode: "included" })
    .mockResolvedValueOnce({ ...candidate, candidateContentBase64: "IyBPdXRjb21lCgpQcmVwYXJlZCBoZXJlLgo=" });

  const user = userEvent.setup();
  const { container } = render(<MissionOutcomePreparation missionId={missionId} />);
  const policyDefault = screen.getByRole("radio", { name: /Use Policy default/ });
  const includeAppendix = screen.getByRole("radio", { name: /Include verification appendix/ });
  const prepare = screen.getByRole("button", { name: "Prepare candidate" });
  expect(document.body).toHaveFocus();
  expect(prepare).toBeDisabled();
  expect(invokeMock).not.toHaveBeenCalled();

  await user.tab();
  expect(policyDefault).toHaveFocus();
  await user.keyboard("{ArrowDown}");
  expect(includeAppendix).toHaveFocus();
  expect(includeAppendix).toBeChecked();
  await user.tab();
  expect(prepare).toHaveFocus();
  expect(prepare).toBeEnabled();
  expect(invokeMock).not.toHaveBeenCalled();
  await user.keyboard("{Enter}");
  const view = await screen.findByRole("button", { name: "View candidate document" });
  expect(screen.getByText("Candidate document is not loaded.")).toBeInTheDocument();
  expect(screen.queryByLabelText("Stored candidate text")).toBeNull();
  expect(screen.getByText(candidate.manifestId)).toBeInTheDocument();
  expect(screen.getByText(candidate.candidateArtifactDigest.value)).toBeInTheDocument();
  expect(screen.getByText(candidate.candidateContentDigest.value)).toBeInTheDocument();
  expect(invokeMock.mock.calls).toEqual([
    ["prepare_outcome_candidate", { missionId, appendixRequest: "include" }],
  ]);

  // Ordinary tab order reaches the actual child returned by preparation. The
  // test never focuses or mounts that child directly.
  expect(prepare).toHaveFocus();
  await user.tab();
  expect(view).toHaveFocus();
  expect(invokeMock.mock.calls).toEqual([
    ["prepare_outcome_candidate", { missionId, appendixRequest: "include" }],
  ]);
  await user.keyboard("{Enter}");
  const stored = await screen.findByLabelText("Stored candidate text");
  expect(stored.textContent).toBe("# Outcome\n\nPrepared here.\n");
  expect(invokeMock.mock.calls).toEqual([
    ["prepare_outcome_candidate", { missionId, appendixRequest: "include" }],
    ["read_outcome_candidate_document", { manifestDigest }],
  ]);
  expect(screen.getByText("This is the stored candidate text. Viewing it does not evaluate or publish it."))
    .toBeInTheDocument();
  await user.tab();
  expect(stored).toHaveFocus();
  await expectNoWcag22Violations(container);
});
