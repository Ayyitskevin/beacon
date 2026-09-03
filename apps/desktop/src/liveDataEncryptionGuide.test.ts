import { describe, expect, it } from "vitest";

import {
  COMMITTED_ENCRYPTION_GUIDES,
  encryptionGuide,
  type GuidePlatform,
} from "./liveDataEncryptionGuide";
import * as guideModule from "./liveDataEncryptionGuide";

const GUIDE_PLATFORMS = [
  "linux",
  "macos",
  "windows",
] as const satisfies readonly GuidePlatform[];

// Independent literals are the allowlist. A guide-body edit must change this
// committed list deliberately; deriving it from the production objects would
// let an unsafe edit update both sides of the assertion at once.
const GUIDE_BODY_ALLOWLIST = [
  `Turning on disk encryption on Linux
Linux encrypts whole volumes with LUKS, through the kernel's device-mapper crypt target. Beacon's unknown-only v1 records bounded Linux observations but does not evaluate LUKS coverage.
The reliable path is to choose full-disk encryption while installing the operating system. Every major installer offers it as a checkbox, and it is the only path that needs no data migration.
An installation that is already running can be converted in place, but conversion rewrites every block on the volume and there is no way to undo it partway through.
A third path avoids both: install fresh with encryption enabled, then restore your data from a backup you have already verified.
Converting an existing installation can destroy all data on the volume if it is interrupted or if the backup you restore from turns out to be incomplete. Verify a restore before you begin.
Follow your distribution's own disk-encryption documentation and the cryptsetup project's LUKS documentation. Beacon does not perform any of these steps for you.`,
  `Turning on disk encryption on macOS
macOS encrypts the startup volume with FileVault.
Open System Settings, then Privacy & Security, then FileVault, and turn it on.
Record the recovery key macOS offers somewhere you can reach without this computer.
Losing the FileVault recovery key without another unlock method makes the volume unreadable.
Apple publishes FileVault setup in its own macOS User Guide. Beacon does not detect FileVault yet and will keep reporting this volume as unknown.`,
  `Turning on disk encryption on Windows
Windows encrypts volumes with BitLocker, or with Device Encryption on editions that do not include BitLocker.
Open Settings, then Privacy & security, then Device encryption or BitLocker drive encryption, and turn it on.
Save the recovery key somewhere you can reach without this computer.
Losing the BitLocker recovery key without another unlock method makes the volume unreadable.
Microsoft publishes BitLocker setup in its own Windows support documentation. Beacon does not detect BitLocker yet and will keep reporting this volume as unknown.`,
] as const;

const RUNNABLE_COMMAND_CONTROLS = [
  "cryptsetup --batch-mode luksFormat /dev/sda",
  String.raw`crypt\setup --batch-mode luksFormat /dev/sda`,
  "crypt'setup' --batch-mode luksFormat /dev/sda",
  String.raw`cr\ypt\setup -q luksErase /dev/nvme0n1p3`,
  `crypt"set"up --debug reencrypt /dev/mapper/data`,
] as const;

function guideBody(guide: ReturnType<typeof encryptionGuide>): string {
  return [guide.title, ...guide.steps, guide.caution, guide.documentation].join(
    "\n",
  );
}

describe("the bundled live-data encryption guide", () => {
  it("describes only the reading unknown-only v1 actually makes", () => {
    const linux = encryptionGuide("linux").steps.join(" ");

    expect(linux).toContain(
      "unknown-only v1 records bounded Linux observations",
    );
    expect(linux).toContain("does not evaluate LUKS coverage");
    expect(linux).not.toContain("Beacon detects exactly that");
  });

  it("renders only the exact committed static guide objects and bodies", () => {
    expect(COMMITTED_ENCRYPTION_GUIDES).toHaveLength(GUIDE_PLATFORMS.length);

    GUIDE_PLATFORMS.forEach((platform, index) => {
      const rendered = encryptionGuide(platform);

      expect(rendered).toBe(COMMITTED_ENCRYPTION_GUIDES[index]);
      expect(GUIDE_BODY_ALLOWLIST).toContain(guideBody(rendered));
    });
  });

  it("exports selection but no runtime guide-construction door", () => {
    expect(Object.keys(guideModule).sort()).toEqual([
      "COMMITTED_ENCRYPTION_GUIDES",
      "detectGuidePlatform",
      "encryptionGuide",
    ]);
  });

  it.each(RUNNABLE_COMMAND_CONTROLS)(
    "keeps the runnable command spelling %s outside every committed body",
    (command) => {
      for (const body of GUIDE_BODY_ALLOWLIST) {
        expect(body).not.toContain(command);
      }

      const planted = {
        ...COMMITTED_ENCRYPTION_GUIDES[0],
        steps: [command],
      };
      expect(GUIDE_BODY_ALLOWLIST).not.toContain(guideBody(planted));
    },
  );
});
