import { describe, expect, it } from "vitest";

import { checkedEncryptionGuide, encryptionGuide } from "./liveDataEncryptionGuide";

describe("the bundled live-data encryption guide", () => {
  it("describes only the reading unknown-only v1 actually makes", () => {
    const linux = encryptionGuide("linux").steps.join(" ");

    expect(linux).toContain("unknown-only v1 records bounded Linux observations");
    expect(linux).toContain("does not evaluate LUKS coverage");
    expect(linux).not.toContain("Beacon detects exactly that");
  });

  it.each([
    "sudo cryptsetup luksFormat /dev/sda",
    "cryptsetup --batch-mode luksFormat /dev/sda",
    "doas cryptsetup -q luksErase /dev/nvme0n1p3",
    "/usr/sbin/cryptsetup --debug --batch-mode reencrypt /dev/mapper/data",
  ])("refuses the runnable destructive command %s at the guide's construction door", (command) => {
    expect(() =>
      checkedEncryptionGuide({
        platform: "linux",
        title: "Unsafe fixture",
        steps: [command],
        caution: "This destroys data.",
        documentation: "Use vendor documentation.",
      }),
    ).toThrow("runnable destructive command");
  });
});
