import { describe, expect, it } from "vitest";

import { checkedEncryptionGuide, encryptionGuide } from "./liveDataEncryptionGuide";

describe("the bundled live-data encryption guide", () => {
  it("describes only the reading unknown-only v1 actually makes", () => {
    const linux = encryptionGuide("linux").steps.join(" ");

    expect(linux).toContain("unknown-only v1 records bounded Linux observations");
    expect(linux).toContain("does not evaluate LUKS coverage");
    expect(linux).not.toContain("Beacon detects exactly that");
  });

  it("refuses a runnable destructive command at the guide's construction door", () => {
    expect(() =>
      checkedEncryptionGuide({
        platform: "linux",
        title: "Unsafe fixture",
        steps: ["sudo cryptsetup luksFormat /dev/sda"],
        caution: "This destroys data.",
        documentation: "Use vendor documentation.",
      }),
    ).toThrow("runnable destructive command");
  });
});
