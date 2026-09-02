import type { LiveDataEncryptionConclusion, SubjectRole } from "./session";

/** The human sentence, rendered from a conclusion by a total function.
 *
 *  It mirrors `render_conclusion` in `crates/beacond/src/disk_encryption.rs`,
 *  and it cannot overflow because its inputs cannot: one fixed template per
 *  reason, at most three subjects, each at most 24 characters. That is the
 *  point of the shape. Five rounds of review found the same defect in a new
 *  place each time -- a sentence composed from unbounded parts, then cut --
 *  and every fix bounded the strings rather than the type that admitted them.
 */
export function renderConclusion(conclusion: LiveDataEncryptionConclusion): string {
  const subjectsForRole = (role: SubjectRole): string => {
    const named = conclusion.subjects
      .filter((subject) => subject.role === role)
      .map((subject) => (subject.shortened ? `${subject.value}…` : subject.value));
    return named.length === 0 ? "an unnamed subject" : named.join(", ");
  };
  const more =
    conclusion.omittedSubjects === 0
      ? ""
      : conclusion.omittedSubjects === 1
        ? " and 1 more"
        : ` and ${conclusion.omittedSubjects} more`;

  let sentence: string;
  switch (conclusion.reason) {
    case "crypt_covers_every_path":
      sentence = `every path to disk passes through ${subjectsForRole("mechanism")}`;
      break;
    case "no_crypt_on_any_path":
      sentence = `at least one path to disk has no crypt layer: ${subjectsForRole("device")}`;
      break;
    case "some_paths_uncovered":
      sentence = `${subjectsForRole("mapping")} is encrypted; ${subjectsForRole("device")} is not`;
      break;
    case "store_path_unresolved":
      sentence = `the live store path could not be resolved: ${subjectsForRole("detail")}`;
      break;
    case "store_not_inspectable":
      sentence = "the live store could not be inspected";
      break;
    case "mount_table_unreadable":
      sentence = "the mount table could not be read";
      break;
    case "mount_table_unparsed":
      sentence = "the mount table could not be parsed";
      break;
    case "mount_table_empty":
      sentence = "the mount table was empty";
      break;
    case "no_mount_for_store":
      sentence = "no mount table entry covers the live store";
      break;
    case "mount_and_stat_disagree":
      sentence = "the mount table and the live store disagree about which volume holds it";
      break;
    case "memory_backed_filesystem":
      sentence = "the live store is on a memory-backed filesystem";
      break;
    case "overlay_filesystem":
      sentence =
        "the live store is on an overlay filesystem; its layers are decided outside this mount";
      break;
    case "userspace_filesystem":
      sentence = "the live store is on a userspace filesystem";
      break;
    case "network_filesystem":
      sentence =
        "the live store is on a network filesystem; its encryption is decided off this host";
      break;
    case "no_block_device_for_filesystem":
      sentence = `this filesystem (${subjectsForRole("location")}) exposes no block device Beacon can inspect`;
      break;
    case "link_resolves_to_nothing":
      sentence = `${subjectsForRole("location")} is a link that resolves to nothing`;
      break;
    case "device_unreadable":
      sentence = `a device in the volume's stack could not be read: ${subjectsForRole("location")}`;
      break;
    case "read_over_budget":
      sentence = `${subjectsForRole("location")} exceeded Beacon's read budget`;
      break;
    case "stack_over_budget":
      sentence = "the volume's device stack exceeded Beacon's read budget";
      break;
    case "no_physical_leaf":
      sentence = "no physical device terminated the volume's stack";
      break;
    case "mapping_without_backing_device":
      sentence = `mapping ${subjectsForRole("mapping")} has no readable backing device`;
      break;
    case "loopback_image":
      sentence = `the volume is a loopback image backed by ${subjectsForRole("location")}; Beacon does not follow the backing file`;
      break;
    case "platform_not_supported":
      sentence = "not detectable on this platform";
      break;
    case "initial_read_failed":
      sentence = `the disk-encryption reading did not complete: ${subjectsForRole("detail")}`;
      break;
  }
  return `${sentence}${more}`;
}

/** The crypt mechanisms the reading named, for the `enabled` status line. */
export function renderMechanisms(conclusion: LiveDataEncryptionConclusion): string {
  return conclusion.subjects
    .filter((subject) => subject.role === "mechanism")
    .map((subject) => (subject.shortened ? `${subject.value}…` : subject.value))
    .join(", ");
}
