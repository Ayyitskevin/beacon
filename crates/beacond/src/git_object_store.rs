//! Dependency-free presence checks against a Git repository's object store.
//!
//! # What this module answers, and what it deliberately does not
//!
//! It answers one question: **is this object present in this repository's object
//! store?** It does not answer whether the object is reachable from any ref.
//! Reachability requires decompressing commit objects and walking their parents,
//! which needs a zlib dependency this crate does not have and a Product Steward
//! decision it has not been given. ADR 0027 also forbids spawning a process, so
//! shelling out to `git` is not an option either.
//!
//! Presence and reachability are different facts on real repositories: a commit
//! left behind by a rebase is present and unreachable. Every caller must record
//! the difference rather than let a presence check read as a reachability claim.
//!
//! Nothing here decompresses data, opens a `.pack` file, or reads outside the
//! repository's `.git` directory.

use std::fs;
use std::path::{Path, PathBuf};

/// Why a repository could not be inspected.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ObjectStoreError {
    /// The path does not exist, is not readable, or is not a Git repository.
    NotARepository,
    /// A `.git` file pointed at another `gitdir`, which itself was indirect.
    /// One level of indirection is supported; a chain is refused rather than
    /// followed to an unbounded depth.
    IndirectionTooDeep,
    /// A pack index existed but could not be parsed as a version 2 index.
    UnreadableIndex,
    /// The inspection budget elapsed before the answer was known. This is not a
    /// negative answer: the caller must keep the two apart.
    Elapsed,
}

/// A repository resolved to its object directory.
#[derive(Debug, Clone)]
pub(crate) struct ObjectStore {
    root: PathBuf,
    objects: PathBuf,
}

impl ObjectStore {
    /// Resolve `source` to its object directory.
    ///
    /// Accepts a normal repository (`<source>/.git/`) and a worktree or
    /// submodule whose `.git` is a file containing `gitdir: <path>`. A bare
    /// repository (`<source>/objects/`) is accepted too. Anything else is
    /// [`ObjectStoreError::NotARepository`].
    pub(crate) fn open(source: &Path) -> Result<Self, ObjectStoreError> {
        let metadata = fs::metadata(source).map_err(|_| ObjectStoreError::NotARepository)?;
        if !metadata.is_dir() {
            return Err(ObjectStoreError::NotARepository);
        }
        let dot_git = source.join(".git");
        let git_dir = match fs::metadata(&dot_git) {
            Ok(metadata) if metadata.is_dir() => dot_git,
            Ok(metadata) if metadata.is_file() => {
                let contents =
                    fs::read_to_string(&dot_git).map_err(|_| ObjectStoreError::NotARepository)?;
                let target = contents
                    .lines()
                    .find_map(|line| line.strip_prefix("gitdir:"))
                    .ok_or(ObjectStoreError::NotARepository)?
                    .trim();
                if target.is_empty() {
                    return Err(ObjectStoreError::NotARepository);
                }
                let target = if Path::new(target).is_absolute() {
                    PathBuf::from(target)
                } else {
                    source.join(target)
                };
                // One level only. A `gitdir:` whose target is itself a `.git`
                // file is refused rather than followed.
                if fs::metadata(target.join(".git")).is_ok() {
                    return Err(ObjectStoreError::IndirectionTooDeep);
                }
                if !fs::metadata(&target).map(|m| m.is_dir()).unwrap_or(false) {
                    return Err(ObjectStoreError::NotARepository);
                }
                target
            }
            // A bare repository keeps `objects/` at its root.
            _ if fs::metadata(source.join("objects"))
                .map(|m| m.is_dir())
                .unwrap_or(false) =>
            {
                source.to_path_buf()
            }
            _ => return Err(ObjectStoreError::NotARepository),
        };
        let objects = git_dir.join("objects");
        if !fs::metadata(&objects).map(|m| m.is_dir()).unwrap_or(false) {
            return Err(ObjectStoreError::NotARepository);
        }
        Ok(Self {
            root: source.to_path_buf(),
            objects,
        })
    }

    /// The repository's working root, canonicalized. Used to decide containment.
    pub(crate) fn canonical_root(&self) -> Result<PathBuf, ObjectStoreError> {
        fs::canonicalize(&self.root).map_err(|_| ObjectStoreError::NotARepository)
    }

    /// Is `revision` (40 lowercase hex characters) present in this object store?
    ///
    /// Checks loose objects first, then every pack index. `deadline` is consulted
    /// after every blocking filesystem call and before every definite answer, so
    /// a pathological store yields [`ObjectStoreError::Elapsed`] rather than a
    /// presence or absence derived from work that finished too late. It cannot
    /// interrupt a call already in progress: the overrun is bounded by one
    /// filesystem operation, not eliminated.
    pub(crate) fn contains(
        &self,
        revision: &str,
        deadline: &mut impl FnMut() -> bool,
    ) -> Result<bool, ObjectStoreError> {
        let raw = decode_revision(revision).ok_or(ObjectStoreError::NotARepository)?;
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let loose = self.objects.join(&revision[..2]).join(&revision[2..]);
        let loose_present = fs::metadata(&loose).map(|m| m.is_file()).unwrap_or(false);
        // Every definite answer below follows a blocking filesystem call, so the budget is
        // re-checked after each one. A read that finished late describes a store as it was before
        // the answer was due; reporting it as presence or absence turns Beacon's own slowness into
        // a claim about the Operator's repository.
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        if loose_present {
            return Ok(true);
        }
        let pack_dir = self.objects.join("pack");
        let entries = fs::read_dir(&pack_dir);
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let entries = match entries {
            Ok(entries) => entries,
            // No pack directory is normal for a young repository.
            Err(_) => return Ok(false),
        };
        let mut indexes: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "idx"))
            .collect();
        indexes.sort();
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        for index in indexes {
            if deadline() {
                return Err(ObjectStoreError::Elapsed);
            }
            if index_contains(&index, &raw, deadline)? {
                return Ok(true);
            }
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        Ok(false)
    }
}

/// Decode 40 lowercase hex characters into 20 bytes.
fn decode_revision(revision: &str) -> Option<[u8; 20]> {
    if revision.len() != 40 {
        return None;
    }
    let bytes = revision.as_bytes();
    let mut raw = [0u8; 20];
    for (index, slot) in raw.iter_mut().enumerate() {
        let high = hex_value(bytes[index * 2])?;
        let low = hex_value(bytes[index * 2 + 1])?;
        *slot = (high << 4) | low;
    }
    Some(raw)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Binary-search one version 2 pack index for `target`.
///
/// Layout: a 4-byte magic `\xfftOc`, a 4-byte version, a 256-entry fanout table
/// of big-endian u32 cumulative counts, then that many sorted 20-byte object
/// names. The fanout narrows the search to names sharing the first byte; only
/// the name table is read, never the pack itself.
fn index_contains(
    path: &Path,
    target: &[u8; 20],
    deadline: &mut impl FnMut() -> bool,
) -> Result<bool, ObjectStoreError> {
    let data = fs::read(path).map_err(|_| ObjectStoreError::UnreadableIndex)?;
    // The read above blocks for as long as the filesystem takes, and an index whose target bucket
    // is empty reaches `Ok(false)` below without entering the search loop -- so without this check
    // a read that completed after the budget became an absence claim.
    if deadline() {
        return Err(ObjectStoreError::Elapsed);
    }
    const HEADER: usize = 8;
    const FANOUT: usize = 256 * 4;
    if data.len() < HEADER + FANOUT {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    if &data[..4] != b"\xfftOc" || u32::from_be_bytes([data[4], data[5], data[6], data[7]]) != 2 {
        // Version 1 indexes have no magic and a different layout. Refusing is
        // honest; silently reading one as version 2 would invent an answer.
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let fanout_at = |bucket: usize| -> u32 {
        let at = HEADER + bucket * 4;
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    };
    let total = fanout_at(255) as usize;
    let names_at = HEADER + FANOUT;
    if data.len() < names_at + total * 20 {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let first = target[0] as usize;
    let mut low = if first == 0 {
        0
    } else {
        fanout_at(first - 1) as usize
    };
    let mut high = fanout_at(first) as usize;
    if high > total || low > high {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    while low < high {
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let middle = low + (high - low) / 2;
        let at = names_at + middle * 20;
        let name = &data[at..at + 20];
        match name.cmp(target.as_slice()) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => return Ok(true),
        }
    }
    if deadline() {
        return Err(ObjectStoreError::Elapsed);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;

    use uuid::Uuid;

    use super::{ObjectStore, ObjectStoreError, index_contains};

    /// A temporary directory removed on drop.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("beacond-objects-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// The product never spawns a process; a test may, to build a fixture whose
    /// object layout only Git can produce.
    fn git(repository: &PathBuf, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(repository)
            .env("GIT_AUTHOR_NAME", "Beacon Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Beacon Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .status()
            .expect("git is available in the test environment");
        assert!(status.success(), "git {arguments:?} failed");
    }

    fn commit_repository(label: &str) -> (TempDir, String) {
        let directory = TempDir::new(label);
        git(&directory.path, &["init", "--quiet"]);
        fs::write(directory.path.join("file.txt"), "contents").unwrap();
        git(&directory.path, &["add", "file.txt"]);
        git(&directory.path, &["commit", "--quiet", "-m", "one"]);
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&directory.path)
            .output()
            .unwrap();
        let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        (directory, revision)
    }

    /// A minimal but valid version 2 pack index holding one name, whose first byte is `0xaa`.
    ///
    /// Any target whose first byte is below `0xaa` lands in an EMPTY fanout bucket, so the binary
    /// search runs zero iterations. That is the shape that used to escape every deadline check.
    fn one_name_index(first_byte: u8) -> Vec<u8> {
        let mut index = Vec::new();
        index.extend_from_slice(b"\xfftOc");
        index.extend_from_slice(&2_u32.to_be_bytes());
        for bucket in 0..256_usize {
            let cumulative: u32 = if bucket >= first_byte as usize { 1 } else { 0 };
            index.extend_from_slice(&cumulative.to_be_bytes());
        }
        index.extend_from_slice(&[first_byte; 20]);
        index
    }

    /// A read that finishes AFTER the budget must not become an absence claim.
    ///
    /// The index is served through a FIFO, so `fs::read` blocks until the writer closes it. By then
    /// the deadline has expired. With an empty target bucket the search body never runs, so before
    /// this guard the function fell straight through to `Ok(false)` — reporting "the revision is not
    /// in this repository" on the strength of a read that was already stale. An absence is a claim
    /// about the store; a timeout is a claim about Beacon. They are not interchangeable.
    #[test]
    fn a_read_that_completes_after_the_budget_is_elapsed_not_absent() {
        let directory = TempDir::new("fifo-late-read");
        let pack = directory.path.join("objects/pack");
        fs::create_dir_all(&pack).unwrap();
        let fifo = pack.join("pack-late.idx");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo is available in the test environment");
        assert!(made.success(), "mkfifo failed");

        let index = one_name_index(0xaa);
        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            // Opening for write blocks until the reader opens; the reader then blocks until this
            // side closes, so the read finishes strictly after the sleep.
            use std::io::Write;
            let mut file = fs::File::create(&writer_path).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = file.write_all(&index);
        });

        let store = ObjectStore::open(&directory.path).expect("a bare store with objects/");
        let started = std::time::Instant::now();
        // A 100 ms budget the blocking read is guaranteed to outlast.
        let mut deadline = move || started.elapsed().as_millis() > 100;
        // First byte 0x01 is below 0xaa, so its bucket is empty and no search iteration runs.
        let outcome = store.contains(&"01".repeat(20), &mut deadline);
        writer.join().unwrap();

        assert_eq!(
            outcome,
            Err(ObjectStoreError::Elapsed),
            "a stale read was reported as a definite answer"
        );
        assert_ne!(outcome, Ok(false), "elapsed work became an absence claim");
    }

    /// The same stale read, pinned at the function that performs it.
    ///
    /// The test above goes through `contains`, which has its own guard before its final answer, so
    /// it stays green even with both of `index_contains`'s checks removed. This one calls
    /// `index_contains` directly, which narrows what it pins — but not to a single line.
    ///
    /// What it actually pins: `index_contains` honours its deadline across a late read. Two checks
    /// inside it enforce that — the one after `fs::read` and the one before its `Ok(false)` — and
    /// they are **mutually redundant for this fixture**: removing either alone leaves this test
    /// green, and removing both makes it fail with `Ok(false)`. Both are kept as defence in depth
    /// on a path whose failure mode is a false claim about someone else's repository, not because
    /// either is separately load-bearing. Anyone deleting one as redundant should know the suite
    /// will not object; deleting both is what this test exists to stop.
    #[test]
    fn index_contains_reports_a_late_read_as_elapsed_rather_than_returning_false() {
        let directory = TempDir::new("fifo-index-late-read");
        fs::create_dir_all(&directory.path).unwrap();
        let fifo = directory.path.join("pack-late.idx");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo is available in the test environment")
                .success()
        );

        let index = one_name_index(0xaa);
        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            let mut file = fs::File::create(&writer_path).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = file.write_all(&index);
        });

        let started = std::time::Instant::now();
        let mut deadline = move || started.elapsed().as_millis() > 100;
        // First byte 0x01 lands in an empty bucket: zero search iterations, so neither of the
        // in-loop checks can fire and the answer is decided by the two checks around them.
        let outcome = index_contains(&fifo, &[0x01; 20], &mut deadline);
        writer.join().unwrap();

        assert_eq!(outcome, Err(ObjectStoreError::Elapsed));
    }

    fn never_elapsed() -> impl FnMut() -> bool {
        || false
    }

    #[test]
    fn a_loose_object_is_present() {
        let (repository, revision) = commit_repository("loose");
        let store = ObjectStore::open(&repository.path).unwrap();
        assert!(store.contains(&revision, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn an_object_only_in_a_pack_is_present() {
        let (repository, revision) = commit_repository("packed");
        // `gc --prune=now` moves every loose object into a pack, so this asserts
        // the index reader rather than the loose-object path.
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let loose = repository
            .path
            .join(".git/objects")
            .join(&revision[..2])
            .join(&revision[2..]);
        assert!(!loose.exists(), "fixture did not pack the commit");
        let store = ObjectStore::open(&repository.path).unwrap();
        assert!(store.contains(&revision, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn an_absent_object_is_absent_in_both_layouts() {
        let missing = "0".repeat(40);
        let (loose_repository, _) = commit_repository("absent-loose");
        let store = ObjectStore::open(&loose_repository.path).unwrap();
        assert!(!store.contains(&missing, &mut never_elapsed()).unwrap());

        let (packed_repository, _) = commit_repository("absent-packed");
        git(&packed_repository.path, &["gc", "--quiet", "--prune=now"]);
        let store = ObjectStore::open(&packed_repository.path).unwrap();
        assert!(!store.contains(&missing, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn an_unreachable_object_is_still_present() {
        // The distinction this module exists to be honest about: a commit left
        // behind by a reset is present in the store and reachable from nothing.
        let (repository, first) = commit_repository("unreachable");
        fs::write(repository.path.join("file.txt"), "second").unwrap();
        git(&repository.path, &["add", "file.txt"]);
        git(&repository.path, &["commit", "--quiet", "-m", "two"]);
        git(&repository.path, &["reset", "--hard", "--quiet", &first]);
        let output = Command::new("git")
            .args(["rev-list", "--all"])
            .current_dir(&repository.path)
            .output()
            .unwrap();
        let reachable = String::from_utf8(output.stdout).unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();
        for line in reachable.lines() {
            assert!(store.contains(line, &mut never_elapsed()).unwrap());
        }
        // The abandoned commit is not in `rev-list --all` yet is present.
        let output = Command::new("git")
            .args(["reflog", "--format=%H"])
            .current_dir(&repository.path)
            .output()
            .unwrap();
        let abandoned = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .find(|candidate| !reachable.contains(candidate.as_str()))
            .expect("the reset left an unreachable commit");
        assert!(store.contains(&abandoned, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn a_directory_that_is_not_a_repository_is_refused() {
        let directory = TempDir::new("not-a-repository");
        assert_eq!(
            ObjectStore::open(&directory.path).unwrap_err(),
            ObjectStoreError::NotARepository
        );
    }

    #[test]
    fn a_missing_path_is_refused() {
        let directory = TempDir::new("missing");
        assert_eq!(
            ObjectStore::open(&directory.path.join("nowhere")).unwrap_err(),
            ObjectStoreError::NotARepository
        );
    }

    #[test]
    fn a_dangling_gitdir_file_is_refused() {
        let directory = TempDir::new("dangling-gitdir");
        fs::write(directory.path.join(".git"), "gitdir: ../nowhere\n").unwrap();
        assert_eq!(
            ObjectStore::open(&directory.path).unwrap_err(),
            ObjectStoreError::NotARepository
        );
    }

    #[test]
    fn a_worktree_gitdir_file_is_followed_once() {
        let (repository, revision) = commit_repository("worktree");
        let worktree = TempDir::new("worktree-link");
        let real_git_dir = repository.path.join(".git");
        fs::write(
            worktree.path.join(".git"),
            format!("gitdir: {}\n", real_git_dir.display()),
        )
        .unwrap();
        let store = ObjectStore::open(&worktree.path).unwrap();
        assert!(store.contains(&revision, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn a_malformed_revision_is_refused() {
        let (repository, _) = commit_repository("malformed");
        let store = ObjectStore::open(&repository.path).unwrap();
        for candidate in ["", "abc", &"z".repeat(40), &"A".repeat(40)] {
            assert_eq!(
                store.contains(candidate, &mut never_elapsed()).unwrap_err(),
                ObjectStoreError::NotARepository
            );
        }
    }

    #[test]
    fn a_corrupt_pack_index_is_refused_rather_than_answered() {
        let (repository, revision) = commit_repository("corrupt-index");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack = repository.path.join(".git/objects/pack");
        let index = fs::read_dir(&pack)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .expect("gc produced a pack index");
        // Git writes pack indexes read-only, so the fixture must reopen it.
        let mut permissions = fs::metadata(&index).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&index, permissions).unwrap();
        fs::write(&index, b"not an index").unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn an_elapsed_budget_is_not_a_negative_answer() {
        let (repository, revision) = commit_repository("elapsed");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let store = ObjectStore::open(&repository.path).unwrap();
        let mut elapsed = || true;
        assert_eq!(
            store.contains(&revision, &mut elapsed).unwrap_err(),
            ObjectStoreError::Elapsed
        );
    }
}
