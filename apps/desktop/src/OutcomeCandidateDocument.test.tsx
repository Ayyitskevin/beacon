import { invoke } from "@tauri-apps/api/core";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { OutcomeCandidateDocument } from "./OutcomeCandidateDocument";
import type { PreparedOutcomeCandidate } from "./session";
import { expectNoWcag22Violations } from "./test/axe";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

// MOCKED CONTRACT ONLY, and precisely: the OWNERS exist -- `OutcomeService::manifest` and
// `OutcomeService::candidate` already read and verify these records. What is missing is the ROUTE:
// there is no `read_outcome_candidate_document` LocalCommand and no native command. Every invoke
// below is answered by a double, so these are FRONTEND bars and NOT an end-to-end claim.
const MISSION = "99999999-9999-4999-8999-9999999c7000";
const MANIFEST = "66666666-6666-4666-8666-666666666666";
const MANIFEST_HEX = "a".repeat(64);
const ARTIFACT_HEX = "b".repeat(64);
const CONTENT_HEX = "c".repeat(64);
const TEXT = "# Outcome\n\nA line with <em>literal</em> markup.\n";

function base64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** The held C6 receipt, written here as a literal. Every expectation in these bars comes from THIS
 *  object, never from the reply under test. */
function held(overrides: Partial<PreparedOutcomeCandidate> = {}): PreparedOutcomeCandidate {
  return {
    missionId: MISSION,
    appendixRequest: "include",
    appendixMode: "included",
    manifestId: MANIFEST,
    manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.outcome.manifest.v2", value: MANIFEST_HEX },
    candidateArtifactDigest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: ARTIFACT_HEX },
    candidateContentDigest: { algorithm: "sha256", canonicalizer: "beacon.raw.v1", value: CONTENT_HEX },
    candidateContentBytes: byteLength(TEXT),
    mediaType: "text/markdown; charset=utf-8",
    ...overrides,
  };
}

function reply(text = TEXT, overrides: Record<string, unknown> = {}) {
  return {
    missionId: MISSION,
    manifestId: MANIFEST,
    manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.outcome.manifest.v2", value: MANIFEST_HEX },
    candidateArtifactDigest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: ARTIFACT_HEX },
    candidateContentDigest: { algorithm: "sha256", canonicalizer: "beacon.raw.v1", value: CONTENT_HEX },
    candidateContentBytes: byteLength(text),
    mediaType: "text/markdown; charset=utf-8",
    candidateContentBase64: base64(text),
    ...overrides,
  };
}

beforeEach(() => {
  cleanup();
  invokeMock.mockReset();
});

it("C7.VIEW.AN_EXPLICIT_CLICK_READS_THE_HELD_IDENTITY: one read, one bare digest", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply());
  render(<OutcomeCandidateDocument receipt={held()} />);

  expect(invokeMock, "C7.VIEW.NO_READ_BEFORE_CLICK").not.toHaveBeenCalled();
  expect(screen.getByText("Candidate document is not loaded."), "C7.VIEW.NEUTRAL_UNTIL_ASKED").toBeInTheDocument();

  await user.click(screen.getByRole("button", { name: "View candidate document" }));

  expect(invokeMock.mock.calls, "C7.VIEW.AN_EXPLICIT_CLICK_READS_THE_HELD_IDENTITY: exactly one read").toHaveLength(1);
  expect(invokeMock.mock.calls[0], "C7.VIEW.AN_EXPLICIT_CLICK_READS_THE_HELD_IDENTITY: the held digest, bare").toEqual([
    "read_outcome_candidate_document",
    { manifestDigest: MANIFEST_HEX },
  ]);
  // `collapseWhitespace: false` AND `trim: false`: the stored text is what is on screen, newline
  // for newline INCLUDING the trailing one. The default matcher collapses runs of whitespace and
  // trims the ends, either of which would let a document that lost its line breaks pass.
  expect(
    await screen.findByText(TEXT, { collapseWhitespace: false, trim: false }),
    "C7.VIEW.THE_STORED_TEXT_IS_SHOWN",
  ).toBeInTheDocument();
});

const NOT_FOUND = "That Outcome candidate is not available in this Workspace.";
const UNAVAILABLE = "Beacon could not verify this Outcome candidate.";

function base64Bytes(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

async function view(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "View candidate document" }));
}

it("C7.VIEW.MARKUP_IN_THE_DOCUMENT_IS_TEXT: nothing in the document is evaluated", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply());
  render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);

  await screen.findByText(TEXT, { collapseWhitespace: false, trim: false });
  expect(document.querySelector("em"), "C7.VIEW.MARKUP_IN_THE_DOCUMENT_IS_TEXT: no element was built").toBeNull();
  expect(document.querySelector("h1"), "C7.VIEW.MARKUP_IN_THE_DOCUMENT_IS_TEXT: no Markdown was evaluated").toBeNull();
  const region = screen.getByText(TEXT, { collapseWhitespace: false, trim: false });
  expect(region.tagName, "C7.VIEW.MARKUP_IN_THE_DOCUMENT_IS_TEXT: a literal text region").toBe("PRE");
});

it("C7.VIEW.THE_BYTES_ARE_PRESERVED: multibyte, CRLF and a BOM survive the round trip", async () => {
  for (const [label, text] of [
    ["multibyte", "héllo 🧭 あ\n"],
    ["CRLF", "first\r\nsecond\r\n"],
    ["BOM", "﻿# Outcome\n"],
    ["trailing spaces", "line   \n\n"],
  ] as const) {
    cleanup();
    invokeMock.mockReset();
    const receipt = held({ candidateContentBytes: byteLength(text) });
    invokeMock.mockResolvedValue(reply(text));
    const user = userEvent.setup();
    render(<OutcomeCandidateDocument receipt={receipt} />);
    await view(user);

    const shown = await screen.findByText(text, { collapseWhitespace: false, trim: false });
    expect(shown.textContent, `C7.VIEW.THE_BYTES_ARE_PRESERVED: ${label}`).toBe(text);
    expect(
      new TextEncoder().encode(shown.textContent ?? "").length,
      `C7.VIEW.THE_BYTES_ARE_PRESERVED: ${label} byte witness`,
    ).toBe(byteLength(text));
  }
});

it("C7.VIEW.THE_BOUNDS_ARE_THE_OWNERS: an empty document and a maximal one both read", async () => {
  // Empty: zero bytes is a document, not an absence.
  cleanup();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(reply(""));
  let user = userEvent.setup();
  render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: 0 })} />);
  await view(user);
  expect(
    await screen.findByText("Stored candidate text"),
    "C7.VIEW.THE_BOUNDS_ARE_THE_OWNERS: an empty document still renders its region",
  ).toBeInTheDocument();
  expect(screen.queryByText(UNAVAILABLE), "C7.VIEW.THE_BOUNDS_ARE_THE_OWNERS: empty is not a fault").toBeNull();

  // Maximal: exactly the owner's ceiling, and exactly the encoded ceiling the design computed.
  cleanup();
  invokeMock.mockReset();
  const maximal = "a".repeat(1_048_576);
  const encoded = base64(maximal);
  expect(encoded.length, "C7.VIEW.THE_BOUNDS_ARE_THE_OWNERS: the encoded ceiling").toBe(1_398_104);
  invokeMock.mockResolvedValue(reply(maximal));
  user = userEvent.setup();
  render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: 1_048_576 })} />);
  await view(user);
  const region = await screen.findByLabelText("Stored candidate text");
  expect(region.textContent?.length, "C7.VIEW.THE_BOUNDS_ARE_THE_OWNERS: maximal").toBe(1_048_576);
});

it("C7.VIEW.EVERY_BINDING_IS_THE_HELD_ONE: a reply that answers something else is unavailable", async () => {
  const faults: readonly (readonly [string, Record<string, unknown>])[] = [
    ["another Mission", { missionId: "88888888-8888-4888-8888-888888888888" }],
    ["another manifest id", { manifestId: "77777777-7777-4777-8777-777777777777" }],
    ["another manifest digest", { manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.outcome.manifest.v2", value: "d".repeat(64) } }],
    ["another Artifact digest", { candidateArtifactDigest: { algorithm: "sha256", canonicalizer: "beacon.artifact.v1", value: "d".repeat(64) } }],
    ["another content digest", { candidateContentDigest: { algorithm: "sha256", canonicalizer: "beacon.raw.v1", value: "d".repeat(64) } }],
    ["a foreign canonicalizer", { candidateArtifactDigest: { algorithm: "sha256", canonicalizer: "beacon.raw.v1", value: ARTIFACT_HEX } }],
    ["another byte count", { candidateContentBytes: byteLength(TEXT) + 1 }],
    ["another media type", { mediaType: "text/plain" }],
    ["an oversized byte count", { candidateContentBytes: 1_048_577 }],
    ["a ninth key", { appendixMode: "included" }],
  ];
  for (const [label, overrides] of faults) {
    cleanup();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(reply(TEXT, overrides));
    const user = userEvent.setup();
    render(<OutcomeCandidateDocument receipt={held()} />);
    await view(user);

    expect(await screen.findByText(UNAVAILABLE), `C7.VIEW.EVERY_BINDING_IS_THE_HELD_ONE: ${label}`).toBeInTheDocument();
    expect(screen.queryByText("Stored candidate text"), `C7.VIEW.NOTHING_IS_SHOWN_ON_A_MISMATCH: ${label}`).toBeNull();
    expect(screen.queryByText(NOT_FOUND), `C7.VIEW.A_MISMATCH_IS_NOT_AN_ABSENCE: ${label}`).toBeNull();
  }

  // A missing key is the same answer.
  cleanup();
  invokeMock.mockReset();
  const { mediaType: _dropped, ...withoutMediaType } = reply();
  invokeMock.mockResolvedValue(withoutMediaType);
  const user = userEvent.setup();
  render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);
  expect(await screen.findByText(UNAVAILABLE), "C7.VIEW.EVERY_BINDING_IS_THE_HELD_ONE: a missing key").toBeInTheDocument();
});

it("C7.VIEW.THE_ENCODING_IS_CANONICAL: malformed, non-canonical and over-long bodies refuse", async () => {
  // Each case carries the byte count its own body actually decodes to, so the refusal under test is
  // the only one that can fire. The `QR==` row previously kept TEXT's count and therefore refused on
  // LENGTH -- the row passed while saying something it never tested (root's witness review). The
  // tail-bit propositions now live in their own named bars below; this row is the malformed-family
  // sibling, told truthfully.
  const bodies: readonly (readonly [string, string, number])[] = [
    ["a stray character", "IQ=!", byteLength(TEXT)],
    ["whitespace", `${base64(TEXT).slice(0, 8)}\n${base64(TEXT).slice(8)}`, byteLength(TEXT)],
    ["bad padding", `${base64(TEXT)}=`, byteLength(TEXT)],
    ["non-canonical tail bits, at its own length", "QR==", 1],
    // Refusal only. That it refuses BEFORE decoding is a separate proposition with its own bar and
    // its own instrument; this row does not claim it.
    ["over the encoded ceiling", `${"A".repeat(1_398_104)}AAAA`, byteLength(TEXT)],
  ];
  for (const [label, body, bytes] of bodies) {
    cleanup();
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(reply(TEXT, { candidateContentBase64: body, candidateContentBytes: bytes }));
    const user = userEvent.setup();
    render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: bytes })} />);
    await view(user);

    expect(await screen.findByText(UNAVAILABLE), `C7.VIEW.THE_ENCODING_IS_CANONICAL: ${label}`).toBeInTheDocument();
    expect(screen.queryByText("Stored candidate text"), `C7.VIEW.THE_ENCODING_IS_CANONICAL: ${label} rendered`).toBeNull();
  }
});

it("C7.VIEW.DECODING_IS_FATAL: an invalid UTF-8 sequence is never silently replaced", async () => {
  const invalid = new Uint8Array([0x23, 0x20, 0xff, 0xfe, 0x0a]);
  invokeMock.mockResolvedValue(
    reply(TEXT, { candidateContentBase64: base64Bytes(invalid), candidateContentBytes: invalid.length }),
  );
  const user = userEvent.setup();
  render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: invalid.length })} />);
  await view(user);

  expect(await screen.findByText(UNAVAILABLE), "C7.VIEW.DECODING_IS_FATAL").toBeInTheDocument();
  expect(screen.queryByText(/�/), "C7.VIEW.DECODING_IS_FATAL: no replacement character reached the screen").toBeNull();
  expect(screen.queryByText("Stored candidate text"), "C7.VIEW.DECODING_IS_FATAL: nothing was shown").toBeNull();
});

it("C7.VIEW.THE_TWO_CODES_AND_THE_ACCOUNT_COPY: each keeps its own fixed sentence", async () => {
  const cases: readonly (readonly [Record<string, unknown>, string])[] = [
    [{ code: "outcome_candidate_not_found" }, NOT_FOUND],
    [{ code: "outcome_candidate_unavailable" }, UNAVAILABLE],
    [{ code: "not_authenticated" }, "Unlock Beacon to continue."],
    [{ code: "local_request_failed", message: "private transport detail" }, "Beacon could not complete that local request."],
  ];
  for (const [error, sentence] of cases) {
    cleanup();
    invokeMock.mockReset();
    invokeMock.mockRejectedValue(error);
    const user = userEvent.setup();
    render(<OutcomeCandidateDocument receipt={held()} />);
    await view(user);

    expect(await screen.findByText(sentence), `C7.VIEW.THE_TWO_CODES_AND_THE_ACCOUNT_COPY: ${String(error.code)}`)
      .toBeInTheDocument();
    expect(
      screen.queryByText(/private transport detail|invalid Outcome candidate document/),
      `C7.VIEW.NO_RAW_DIAGNOSTIC: ${String(error.code)}`,
    ).toBeNull();
  }
});

it("C7.VIEW.RETRY_IS_AN_EXPLICIT_READ: it calls the loader again with the same held identity", async () => {
  const user = userEvent.setup();
  invokeMock.mockRejectedValueOnce({ code: "outcome_candidate_unavailable" });
  invokeMock.mockResolvedValue(reply());
  render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);
  expect(await screen.findByText(UNAVAILABLE), "C7.VIEW.RETRY_IS_AN_EXPLICIT_READ control").toBeInTheDocument();
  expect(invokeMock.mock.calls, "C7.VIEW.NO_AUTOMATIC_RETRY").toHaveLength(1);

  await user.click(screen.getByRole("button", { name: "Read again" }));
  expect(invokeMock.mock.calls, "C7.VIEW.RETRY_IS_AN_EXPLICIT_READ: one more read").toHaveLength(2);
  expect(invokeMock.mock.calls[1], "C7.VIEW.RETRY_IS_AN_EXPLICIT_READ: the same held identity").toEqual(
    invokeMock.mock.calls[0],
  );
  expect(await screen.findByText(TEXT, { collapseWhitespace: false, trim: false }), "C7.VIEW.RETRY_SUCCEEDS")
    .toBeInTheDocument();
});

it("C7.VIEW.PENDING_BLOCKS_A_SECOND_READ", async () => {
  const user = userEvent.setup();
  let release: ((value: unknown) => void) | undefined;
  invokeMock.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);

  const button = screen.getByRole("button", { name: "View candidate document" });
  expect(button, "C7.VIEW.PENDING_BLOCKS_A_SECOND_READ").toBeDisabled();
  expect(screen.getByText("Reading candidate document…"), "C7.VIEW.PENDING_IS_VISIBLE").toBeInTheDocument();
  await user.click(button);
  expect(invokeMock.mock.calls, "C7.VIEW.PENDING_BLOCKS_A_SECOND_READ").toHaveLength(1);

  await act(async () => {
    release?.(reply());
    await Promise.resolve();
  });
  expect(await screen.findByText(TEXT, { collapseWhitespace: false, trim: false })).toBeInTheDocument();
});

it("C7.VIEW.A_STALE_REPLY_NEVER_ATTACHES: identity A to B to A, and attempt 1 to 2", async () => {
  const user = userEvent.setup();
  const otherReceipt = held({ manifestDigest: { algorithm: "sha256", canonicalizer: "beacon.outcome.manifest.v2", value: "e".repeat(64) } });
  const releases: ((value: unknown) => void)[] = [];
  invokeMock.mockImplementation(() => new Promise((resolve) => { releases.push(resolve); }));

  const view1 = render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);
  // A -> B -> A: the SAME digest returns, so only a generation can tell the attempts apart.
  view1.rerender(<OutcomeCandidateDocument receipt={otherReceipt} />);
  view1.rerender(<OutcomeCandidateDocument receipt={held()} />);
  await act(async () => {
    releases[0]?.(reply());
    await Promise.resolve();
  });
  expect(screen.queryByText("Stored candidate text"), "C7.VIEW.A_STALE_REPLY_NEVER_ATTACHES: A to B to A").toBeNull();
  expect(
    screen.getByText("Candidate document is not loaded."),
    "C7.VIEW.A_STALE_REPLY_NEVER_ATTACHES: the returning identity inherits nothing",
  ).toBeInTheDocument();

  // Attempt 1 then attempt 2 on ONE identity: the first reply must not land after the second starts.
  await view(user);
  const secondAttempt = releases.length;
  await act(async () => {
    releases[secondAttempt - 1]?.(reply());
    await Promise.resolve();
  });
  expect(
    await screen.findByText(TEXT, { collapseWhitespace: false, trim: false }),
    "C7.VIEW.A_STALE_REPLY_NEVER_ATTACHES: the live attempt lands",
  ).toBeInTheDocument();
});

it("C7.VIEW.UNMOUNT_RETIRES_THE_READ", async () => {
  const user = userEvent.setup();
  let release: ((value: unknown) => void) | undefined;
  invokeMock.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
  const mounted = render(<OutcomeCandidateDocument receipt={held()} />);
  await view(user);
  mounted.unmount();

  await act(async () => {
    release?.(reply());
    await Promise.resolve();
  });
  expect(screen.queryByText("Stored candidate text"), "C7.VIEW.UNMOUNT_RETIRES_THE_READ").toBeNull();
});

it("C7.VIEW.KEYBOARD_REACHES_THE_ACTION_AND_THE_DOCUMENT", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply());
  render(<OutcomeCandidateDocument receipt={held()} />);

  await user.tab();
  const action = screen.getByRole("button", { name: "View candidate document" });
  expect(action, "C7.VIEW.KEYBOARD_REACHES_THE_ACTION_AND_THE_DOCUMENT").toHaveFocus();
  await user.keyboard("{Enter}");
  await screen.findByText(TEXT, { collapseWhitespace: false, trim: false });
  await user.tab();
  expect(
    screen.getByLabelText("Stored candidate text"),
    "C7.VIEW.KEYBOARD_REACHES_THE_ACTION_AND_THE_DOCUMENT: the scrollable region is reachable",
  ).toHaveFocus();
});

it("C7.VIEW.THE_SURFACE_MEETS_WCAG_2_2", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply());
  render(<OutcomeCandidateDocument receipt={held()} />);
  await expectNoWcag22Violations(document.body);
  await view(user);
  await screen.findByText(TEXT, { collapseWhitespace: false, trim: false });
  await expectNoWcag22Violations(document.body);
});


it("astra_c7_one_byte_noncanonical_tail_is_refused_at_its_actual_length", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply("A", { candidateContentBase64: "QR==" }));
  render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: 1 })} />);
  await view(user);
  expect(
    screen.queryByText(UNAVAILABLE) !== null,
    "C7.ASTRA.ONE_BYTE_TAIL_BITS_ARE_CANONICAL",
  ).toBe(true);
  expect(screen.queryByText("Stored candidate text"), "C7.ASTRA.ONE_BYTE_NONCANONICAL_TEXT_NOT_SHOWN").toBeNull();
});

it("astra_c7_two_byte_noncanonical_tail_is_refused_at_its_actual_length", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply("AB", { candidateContentBase64: "QUJ=" }));
  render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: 2 })} />);
  await view(user);
  expect(
    screen.queryByText(UNAVAILABLE) !== null,
    "C7.ASTRA.TWO_BYTE_TAIL_BITS_ARE_CANONICAL",
  ).toBe(true);
  expect(screen.queryByText("Stored candidate text"), "C7.ASTRA.TWO_BYTE_NONCANONICAL_TEXT_NOT_SHOWN").toBeNull();
});

it("astra_c7_the_encoded_ceiling_is_checked_before_decoding", async () => {
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply(TEXT, {
    candidateContentBytes: 1_048_576,
    candidateContentBase64: "A".repeat(1_398_108),
  }));
  const decode = vi.spyOn(globalThis, "atob");
  try {
    render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: 1_048_576 })} />);
    await view(user);
    expect(
      screen.queryByText(UNAVAILABLE) !== null,
      "C7.ASTRA.OVERLIMIT_IS_UNAVAILABLE",
    ).toBe(true);
    expect(decode.mock.calls.length, "C7.ASTRA.ENCODED_LIMIT_PRECEDES_DECODING").toBe(0);
  } finally {
    decode.mockRestore();
  }
});

it("C7.VIEW.THE_CANONICAL_TAILS_ARE_ACCEPTED: the same lengths, encoded canonically, read", async () => {
  // The positive siblings of root's two witnesses. Without these, a guard that refused EVERY padded
  // body would pass them both, and "non-canonical is refused" would be indistinguishable from
  // "padded is refused".
  for (const [label, text, body] of [
    ["one byte", "A", "QQ=="],
    ["two bytes", "AB", "QUI="],
  ] as const) {
    cleanup();
    invokeMock.mockReset();
    expect(base64(text), `C7.VIEW.THE_CANONICAL_TAILS_ARE_ACCEPTED: ${label} fixture is canonical`).toBe(body);
    invokeMock.mockResolvedValue(reply(text, { candidateContentBase64: body }));
    const user = userEvent.setup();
    render(<OutcomeCandidateDocument receipt={held({ candidateContentBytes: byteLength(text) })} />);
    await view(user);

    expect(
      await screen.findByText(text, { collapseWhitespace: false, trim: false }),
      `C7.VIEW.THE_CANONICAL_TAILS_ARE_ACCEPTED: ${label}`,
    ).toBeInTheDocument();
    expect(
      screen.queryByText(UNAVAILABLE) === null,
      `C7.VIEW.THE_CANONICAL_TAILS_ARE_ACCEPTED: ${label} was not refused`,
    ).toBe(true);
  }
});

it("C7.VIEW.THE_DECODER_SPY_OBSERVES_A_REAL_READ: the instrument works before it proves an absence", async () => {
  // Root's ceiling witness asserts the decoder was never called. That assertion is only worth
  // something if this spy can see a call at all -- an instrument that observes nothing passes an
  // absence test for free.
  const user = userEvent.setup();
  invokeMock.mockResolvedValue(reply());
  const decode = vi.spyOn(globalThis, "atob");
  try {
    render(<OutcomeCandidateDocument receipt={held()} />);
    await view(user);
    await screen.findByText(TEXT, { collapseWhitespace: false, trim: false });
    expect(
      decode.mock.calls.length >= 1,
      "C7.VIEW.THE_DECODER_SPY_OBSERVES_A_REAL_READ: the spy saw the real decoder",
    ).toBe(true);
  } finally {
    decode.mockRestore();
  }
});
