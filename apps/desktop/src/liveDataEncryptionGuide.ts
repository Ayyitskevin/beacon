/** Bundled setup guidance for turning on operating-system disk encryption.
 *
 *  Bundled rather than linked: this Installation reports that external network
 *  activity is off, and a warning that opens a browser would contradict it. The
 *  guide therefore ships as text and points at each vendor's documentation by
 *  name so the operator can find it themselves.
 *
 *  Nothing here is a command Beacon runs, and nothing here is a command the
 *  operator can paste into a shell to re-encrypt a live installation. That
 *  operation destroys data without a verified backup, so this page names the
 *  path and stops.
 */

export type GuidePlatform = "linux" | "macos" | "windows";

export type EncryptionGuide = {
  platform: GuidePlatform;
  title: string;
  steps: readonly string[];
  caution: string;
  documentation: string;
};

const guides: Record<GuidePlatform, EncryptionGuide> = {
  linux: {
    platform: "linux",
    title: "Turning on disk encryption on Linux",
    steps: [
      "Linux encrypts whole volumes with LUKS, through the kernel's device-mapper crypt target. Beacon detects exactly that.",
      "The reliable path is to choose full-disk encryption while installing the operating system. Every major installer offers it as a checkbox, and it is the only path that needs no data migration.",
      "An installation that is already running can be converted in place, but conversion rewrites every block on the volume and there is no way to undo it partway through.",
      "A third path avoids both: install fresh with encryption enabled, then restore your data from a backup you have already verified.",
    ],
    caution:
      "Converting an existing installation can destroy all data on the volume if it is interrupted or if the backup you restore from turns out to be incomplete. Verify a restore before you begin.",
    documentation:
      "Follow your distribution's own disk-encryption documentation and the cryptsetup project's LUKS documentation. Beacon does not perform any of these steps for you.",
  },
  macos: {
    platform: "macos",
    title: "Turning on disk encryption on macOS",
    steps: [
      "macOS encrypts the startup volume with FileVault.",
      "Open System Settings, then Privacy & Security, then FileVault, and turn it on.",
      "Record the recovery key macOS offers somewhere you can reach without this computer.",
    ],
    caution:
      "Losing the FileVault recovery key without another unlock method makes the volume unreadable.",
    documentation:
      "Apple publishes FileVault setup in its own macOS User Guide. Beacon does not detect FileVault yet and will keep reporting this volume as unknown.",
  },
  windows: {
    platform: "windows",
    title: "Turning on disk encryption on Windows",
    steps: [
      "Windows encrypts volumes with BitLocker, or with Device Encryption on editions that do not include BitLocker.",
      "Open Settings, then Privacy & security, then Device encryption or BitLocker drive encryption, and turn it on.",
      "Save the recovery key somewhere you can reach without this computer.",
    ],
    caution:
      "Losing the BitLocker recovery key without another unlock method makes the volume unreadable.",
    documentation:
      "Microsoft publishes BitLocker setup in its own Windows support documentation. Beacon does not detect BitLocker yet and will keep reporting this volume as unknown.",
  },
};

/** Which guide to show, from the browser's own platform string.
 *
 *  Every guide states the operating system it is for, so a wrong reading is
 *  visible to the operator rather than silently misleading. Linux is the
 *  default because it is the platform Beacon detects.
 */
export function detectGuidePlatform(userAgent: string): GuidePlatform {
  if (/Mac OS X|Macintosh/i.test(userAgent)) return "macos";
  if (/Windows/i.test(userAgent)) return "windows";
  return "linux";
}

export function encryptionGuide(platform: GuidePlatform): EncryptionGuide {
  return guides[platform];
}
