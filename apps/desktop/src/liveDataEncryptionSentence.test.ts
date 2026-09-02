import { describe, expect, it } from "vitest";

import { renderConclusion, renderMechanisms } from "./liveDataEncryptionSentence";
import {
  MAX_SUBJECTS,
  MAX_SUBJECT_CHARS,
  REASON_CODES,
  SUBJECT_ROLES,
  requiredRoles,
  stateForReason,
} from "./session";
import type { LiveDataEncryptionConclusion, SubjectRole } from "./session";

/** The bound the Rust side asserts, restated here because the panel renders
 *  its own sentence and a bound proved on one side of the wire is not proved
 *  on the other. */
const MAX_RENDERED_CONCLUSION_CHARS = 200;

function widest(role: SubjectRole): string {
  // A device is a shape, so its widest legal value is the widest device number
  // rather than filler at the character bound.
  return role === "device" ? "4294967295:4294967295" : "m".repeat(MAX_SUBJECT_CHARS);
}

function conclusion(
  reason: (typeof REASON_CODES)[number],
  roles: SubjectRole[],
  omittedSubjects: number,
): LiveDataEncryptionConclusion {
  return {
    state: stateForReason(reason),
    reason,
    subjects: roles.map((role) => ({
      role,
      value: widest(role),
      shortened: role !== "device",
    })),
    omittedSubjects,
  };
}

describe("the rendered conclusion", () => {
  it("cannot outgrow its bound, for any reason and any subjects", () => {
    // The property the shape exists for, on the side that renders it for an
    // operator. The Rust property proves the same thing for logs; neither
    // proves the other, and round six had only the Rust half.
    let checked = 0;
    for (const reason of REASON_CODES) {
      for (let combination = 0; combination < SUBJECT_ROLES.length ** MAX_SUBJECTS; combination++) {
        const roles: SubjectRole[] = [];
        let index = combination;
        for (let slot = 0; slot < MAX_SUBJECTS; slot++) {
          roles.push(SUBJECT_ROLES[index % SUBJECT_ROLES.length]);
          index = Math.floor(index / SUBJECT_ROLES.length);
        }
        // The maximum omission count a `u32` can carry, which is the longest
        // " and N more" the sentence can gain.
        const rendered = renderConclusion(conclusion(reason, roles, 4_294_967_295));
        checked += 1;
        expect(rendered.length).toBeLessThanOrEqual(MAX_RENDERED_CONCLUSION_CHARS);
        expect(rendered.length).toBeGreaterThan(0);
      }
    }
    expect(checked).toBe(REASON_CODES.length * SUBJECT_ROLES.length ** MAX_SUBJECTS);
  });

  it("names every role it declares, so no reason renders an unnamed subject", () => {
    // The other half: a conclusion carrying the roles the adapter requires
    // never falls back to "an unnamed subject". The fallback stays in the
    // renderer because the function must be total; it must be unreachable for
    // any conclusion the adapter admits.
    for (const reason of REASON_CODES) {
      const roles = requiredRoles(reason);
      const rendered = renderConclusion(conclusion(reason, [...roles], 0));
      if (roles.length > 0) {
        expect(rendered).not.toContain("an unnamed subject");
      }
      for (const role of roles) {
        expect(rendered).toContain(widest(role).slice(0, 8));
      }
    }
  });

  it("marks a shortened subject from its flag in both renderers", () => {
    const shortened = conclusion("crypt_covers_every_path", ["mechanism"], 0);
    expect(renderMechanisms(shortened)).toBe(`${"m".repeat(MAX_SUBJECT_CHARS)}…`);
    const whole: LiveDataEncryptionConclusion = {
      ...shortened,
      subjects: [{ role: "mechanism", value: "dm-crypt (LUKS2)", shortened: false }],
    };
    expect(renderMechanisms(whole)).toBe("dm-crypt (LUKS2)");
    expect(renderConclusion(whole)).not.toContain("…");
  });
});
