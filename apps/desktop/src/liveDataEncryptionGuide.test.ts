import { describe, expect, it } from "vitest";

import { encryptionGuide } from "./liveDataEncryptionGuide";

describe("the bundled live-data encryption guide", () => {
  it("describes only the reading unknown-only v1 actually makes", () => {
    const linux = encryptionGuide("linux").steps.join(" ");

    expect(linux).toContain("unknown-only v1 records bounded Linux observations");
    expect(linux).toContain("does not evaluate LUKS coverage");
    expect(linux).not.toContain("Beacon detects exactly that");
  });
});
