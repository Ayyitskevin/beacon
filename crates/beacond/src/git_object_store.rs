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
//! It also reads one loose object at a time, on request: [`ObjectStore::read_loose_object`]
//! inflates a loose file, proves it is the object its own name hashes to, and hands back the body.
//! That is a bounded read of one named object, not the reachability walk above -- it still cannot
//! say what a ref points at, and it still reads nothing that is packed.
//!
//! Until #33 this module decompressed nothing, and its doc said so. The reader arrived because the
//! readable-context comparison needs object CONTENT, not presence, and a second inflate-and-verify
//! path beside `sample_project`'s would have been two chances to get the self-authentication wrong.
//!
//! Nothing here reads outside the repository's `.git` directory. Files are opened relative to
//! retained directory descriptors with `O_NOFOLLOW`; names are never accepted as file identity.

use std::ffi::{CStr, CString, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use data_encoding::HEXLOWER;
use flate2::{Decompress, FlushDecompress, Status};
use sha1::{Digest, Sha1};

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

/// The four object types a loose read may name. `tag` is in the vocabulary and parsed; a caller
/// that does not expect one refuses it explicitly rather than meeting an unnamed type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl ObjectKind {
    fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"commit" => Some(Self::Commit),
            b"tree" => Some(Self::Tree),
            b"blob" => Some(Self::Blob),
            b"tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// What one loose-object read established. Four outcomes, kept apart on purpose.
///
/// `NotLoose` is **not** a claim that the object is absent. A loose file may be missing because the
/// object is packed, because it was never written, or because a repack is running, and only an
/// authenticated pack lookup tells those apart. A caller that reports `NotLoose` as absence is
/// making a claim this read did not establish.
///
/// `TooLarge` is a resource refusal, not a broken object: the file is there and may be perfectly
/// well formed, and this read declined to hold it. `Malformed` is the broken answer. Collapsing the
/// two would describe Beacon's own bound as the Operator's repository being corrupt.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LooseRead {
    /// Inflated, header-parsed, and self-authenticated against the object's own name.
    Read {
        kind: ObjectKind,
        body: Vec<u8>,
    },
    NotLoose,
    TooLarge,
    Malformed,
}

/// The modes a Git tree entry may carry. Anything else is a malformed tree, not a new kind of
/// entry: the set is closed in the format, so an unknown mode is a parse failure rather than
/// something to describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryMode {
    Directory,
    RegularFile,
    ExecutableFile,
    Symlink,
    Gitlink,
}

impl EntryMode {
    /// Raw tree bytes, not the pretty-printed form. `git cat-file -p` shows a directory as
    /// `040000`; the bytes on disk are `40000`, five characters with no leading zero. A parser
    /// written from the display refuses every directory that exists.
    fn parse(value: &[u8]) -> Option<Self> {
        match value {
            b"40000" => Some(Self::Directory),
            b"100644" => Some(Self::RegularFile),
            b"100755" => Some(Self::ExecutableFile),
            b"120000" => Some(Self::Symlink),
            b"160000" => Some(Self::Gitlink),
            _ => None,
        }
    }

    fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }

    fn is_regular_file(self) -> bool {
        matches!(self, Self::RegularFile | Self::ExecutableFile)
    }
}

/// One parsed tree entry, borrowed from the tree body it was read out of.
#[derive(Debug)]
struct TreeEntry<'a> {
    mode: EntryMode,
    name: &'a [u8],
    id: [u8; 20],
}

/// What resolving one repository-relative path at one revision established.
///
/// Six outcomes, and the distinctions between them are the whole point. `Absent` is a claim about
/// the repository -- the containing tree was read in full, authenticated, and does not hold that
/// name -- while `NotLoose`, `TooLarge` and `Malformed` are claims about what this reader could do,
/// carried up from whichever object on the path produced them. A caller that collapsed the two
/// groups would report Beacon's own limits as facts about the Operator's history.
///
/// `NotARegularFile` is deliberately its own arm and deliberately not a mismatch here: git stores a
/// `120000` symlink as a blob holding the link text, so the mode alone does not establish which
/// bytes anyone presented. What that means for readiness is the caller's decision, and ADR 0034
/// does not yet define it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PathResolution {
    /// The path named a regular file, whose object was read and self-authenticated. The body is
    /// the file's exact bytes, with the object header already stripped.
    Blob { body: Vec<u8> },
    /// Every object on the path was read, and the path is not in the tree at that revision.
    Absent,
    /// The path resolved to an entry that is not a regular file: a symlink, a gitlink, or a
    /// directory where a file was named.
    NotARegularFile,
    /// Some object on the path is not loose. Not an absence -- see [`LooseRead::NotLoose`].
    NotLoose,
    /// Some object on the path exceeded the caller's bound.
    TooLarge,
    /// Some object on the path did not inflate, did not authenticate, did not parse, or was not
    /// the type its position requires.
    Malformed,
}

/// How deep a declared path may go before this reader refuses to keep descending.
///
/// A bound on the walk, not a statement about repositories: paths this deep exist, and one that
/// hits the cap is `Malformed` rather than absent, because the walk did not finish. The declared
/// path is already validated by its own module; this is the second, independent limit that keeps a
/// hostile tree from choosing how much work Beacon does.
const MAX_PATH_COMPONENTS: usize = 64;

/// A repository resolved to its object directory.
#[derive(Debug)]
pub(crate) struct ObjectStore {
    root: PathBuf,
    objects: File,
}

impl ObjectStore {
    /// Resolve `source` to its object directory.
    ///
    /// Accepts a normal repository (`<source>/.git/`) and a worktree or
    /// submodule whose `.git` is a file containing `gitdir: <path>`. A bare
    /// repository (`<source>/objects/`) is accepted too. Anything else is
    /// [`ObjectStoreError::NotARepository`].
    pub(crate) fn open(source: &Path) -> Result<Self, ObjectStoreError> {
        let metadata =
            fs::symlink_metadata(source).map_err(|_| ObjectStoreError::NotARepository)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ObjectStoreError::NotARepository);
        }
        let source_directory =
            open_directory_path(source).map_err(|_| ObjectStoreError::NotARepository)?;
        let git_directory = match fs::symlink_metadata(source.join(".git")) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {
                open_directory_at(&source_directory, b".git")
                    .map_err(|_| ObjectStoreError::NotARepository)?
                    .ok_or(ObjectStoreError::NotARepository)?
            }
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
                let mut marker = open_regular_at(&source_directory, b".git")
                    .map_err(|_| ObjectStoreError::NotARepository)?
                    .ok_or(ObjectStoreError::NotARepository)?;
                let mut contents = String::new();
                marker
                    .by_ref()
                    .take(4097)
                    .read_to_string(&mut contents)
                    .map_err(|_| ObjectStoreError::NotARepository)?;
                if contents.len() > 4096 {
                    return Err(ObjectStoreError::NotARepository);
                }
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
                if fs::symlink_metadata(target.join(".git")).is_ok() {
                    return Err(ObjectStoreError::IndirectionTooDeep);
                }
                open_directory_path(&target).map_err(|_| ObjectStoreError::NotARepository)?
            }
            // A bare repository keeps `objects/` at its root.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => source_directory,
            _ => return Err(ObjectStoreError::NotARepository),
        };
        let objects = open_directory_at(&git_directory, b"objects")
            .map_err(|_| ObjectStoreError::NotARepository)?
            .ok_or(ObjectStoreError::NotARepository)?;
        Ok(Self {
            root: source.to_path_buf(),
            objects,
        })
    }

    /// The repository's working root, canonicalized. Used to decide containment.
    pub(crate) fn canonical_root(&self) -> Result<PathBuf, ObjectStoreError> {
        fs::canonicalize(&self.root).map_err(|_| ObjectStoreError::NotARepository)
    }

    /// Read one loose object by its 40-hex name.
    ///
    /// `maximum` bounds both the compressed file and the inflated object, and is the CALLER's
    /// bound: this function invents no limit of its own, because a limit chosen here would be a
    /// policy about how large a legitimate file may be, applied where nothing knows what the file
    /// is for. Exceeding it is [`LooseRead::TooLarge`] -- never an error, never an absence.
    ///
    /// Every open is descriptor-relative through the retained object directory with `O_NOFOLLOW`,
    /// so the read cannot be redirected by a symlink swapped in between two path resolutions. The
    /// path-based shape `sample_project::read_small_regular` used is deliberately not inherited.
    ///
    /// The self-authentication is what makes the returned bytes usable at all: a Git object's name
    /// is the SHA-1 of its uncompressed `<type> <size>\0<body>` form, so an object that hashes to
    /// its own filename cannot have been substituted for another. Bytes that fail it are
    /// `Malformed`, and no caller ever sees them.
    ///
    /// `deadline` is consulted after each blocking step and before the definite answer, for the
    /// reason [`ObjectStore::contains`] gives: a read that finished late describes the store as it
    /// was before the answer was due.
    pub(crate) fn read_loose_object(
        &self,
        object_id: &str,
        maximum: usize,
        deadline: &mut impl FnMut() -> bool,
    ) -> Result<LooseRead, ObjectStoreError> {
        let raw_id = decode_revision(object_id).ok_or(ObjectStoreError::NotARepository)?;
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let Some(prefix) = open_directory_at(&self.objects, &object_id.as_bytes()[..2])
            .map_err(|_| ObjectStoreError::NotARepository)?
        else {
            return settled(LooseRead::NotLoose, deadline);
        };
        let Some(mut file) = open_regular_at(&prefix, &object_id.as_bytes()[2..])
            .map_err(|_| ObjectStoreError::NotARepository)?
        else {
            return settled(LooseRead::NotLoose, deadline);
        };
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        // Sized from the descriptor already open, never from the path: a stat-then-open pair
        // answers about a file that need not be the one read.
        let length = file
            .metadata()
            .map_err(|_| ObjectStoreError::NotARepository)?
            .len();
        if length > maximum as u64 {
            return settled(LooseRead::TooLarge, deadline);
        }
        let mut compressed = Vec::with_capacity(length as usize);
        file.by_ref()
            .take(length)
            .read_to_end(&mut compressed)
            .map_err(|_| ObjectStoreError::NotARepository)?;
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }

        // Capacity is the inflation bound: `decompress_vec` will not grow past it, so an object
        // that wants more stops short of `StreamEnd`. That is why the capacity is `maximum + 1`
        // and why the status check below is a size guard as well as a completeness one.
        let mut decoder = Decompress::new(true);
        let mut object = Vec::with_capacity(maximum.saturating_add(1));
        let status = match decoder.decompress_vec(&compressed, &mut object, FlushDecompress::Finish)
        {
            Ok(status) => status,
            Err(_) => return settled(LooseRead::Malformed, deadline),
        };
        if status != Status::StreamEnd || decoder.total_in() != compressed.len() as u64 {
            // Either the stream did not finish -- which includes hitting the inflation bound --
            // or bytes trail it. A loose object file holds one zlib stream and nothing after it.
            return settled(
                if object.len() > maximum {
                    LooseRead::TooLarge
                } else {
                    LooseRead::Malformed
                },
                deadline,
            );
        }
        if object.len() > maximum {
            return settled(LooseRead::TooLarge, deadline);
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }

        // The name authenticates the whole uncompressed form, header included, so this runs before
        // the header is trusted for anything.
        if Sha1::digest(&object)[..] != raw_id {
            return settled(LooseRead::Malformed, deadline);
        }
        let Some(header_end) = object.iter().position(|byte| *byte == 0) else {
            return settled(LooseRead::Malformed, deadline);
        };
        let header = &object[..header_end];
        let Some(type_end) = header.iter().position(|byte| *byte == b' ') else {
            return settled(LooseRead::Malformed, deadline);
        };
        let Some(kind) = ObjectKind::parse(&header[..type_end]) else {
            return settled(LooseRead::Malformed, deadline);
        };
        let body = &object[header_end + 1..];
        if parse_object_size(&header[type_end + 1..]) != Some(body.len()) {
            return settled(LooseRead::Malformed, deadline);
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        settled(
            LooseRead::Read {
                kind,
                body: body.to_vec(),
            },
            deadline,
        )
    }

    /// Resolve one repository-relative path to its file bytes at `revision`.
    ///
    /// Walks commit -> tree -> ... -> blob, reading every object through
    /// [`ObjectStore::read_loose_object`], so every object on the path is inflated and proved to
    /// be the object its own name hashes to before anything is concluded from it. Nothing here
    /// reads a packed object; a path that crosses one is [`PathResolution::NotLoose`] and no
    /// stronger claim is made about it.
    ///
    /// `maximum` bounds each object individually, as the reader's own bound does; the walk adds a
    /// depth bound of its own ([`MAX_PATH_COMPONENTS`]) so a tree cannot choose how far Beacon
    /// descends.
    ///
    /// The path is expected to be repository-relative and already validated by its owning module.
    /// This function re-checks the shape it depends on -- non-empty components, no `.` or `..`, no
    /// NUL -- rather than trusting a caller's validation, because a component that reached here
    /// malformed would be compared against tree entries as if it were a name.
    // Nothing calls this until the comparison is wired, and clippy is right to say so: a
    // `pub(crate)` walk with no caller is dead code. `expect` rather than `allow` because it fails
    // once the lint stops firing, so the slice that adds the caller cannot leave the suppression
    // behind -- the compiler removes it rather than a reviewer having to notice.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the comparison that calls this is slice 3")
    )]
    pub(crate) fn resolve_path_at(
        &self,
        revision: &str,
        path: &str,
        maximum: usize,
        deadline: &mut impl FnMut() -> bool,
    ) -> Result<PathResolution, ObjectStoreError> {
        let components: Vec<&str> = path.split('/').collect();
        if components.is_empty()
            || components.len() > MAX_PATH_COMPONENTS
            || components.iter().any(|component| {
                component.is_empty()
                    || *component == "."
                    || *component == ".."
                    || component.contains('\0')
            })
        {
            return settled(PathResolution::Malformed, deadline);
        }

        // The commit names exactly one tree, and its own body is what says so.
        let mut tree_id = match self.read_loose_object(revision, maximum, deadline)? {
            LooseRead::Read {
                kind: ObjectKind::Commit,
                body,
            } => match commit_tree_id(&body) {
                Some(id) => id,
                None => return settled(PathResolution::Malformed, deadline),
            },
            LooseRead::Read { .. } => return settled(PathResolution::Malformed, deadline),
            LooseRead::NotLoose => return settled(PathResolution::NotLoose, deadline),
            LooseRead::TooLarge => return settled(PathResolution::TooLarge, deadline),
            LooseRead::Malformed => return settled(PathResolution::Malformed, deadline),
        };

        for (index, component) in components.iter().enumerate() {
            let last = index + 1 == components.len();
            let body = match self.read_loose_object(&tree_id, maximum, deadline)? {
                LooseRead::Read {
                    kind: ObjectKind::Tree,
                    body,
                } => body,
                LooseRead::Read { .. } => return settled(PathResolution::Malformed, deadline),
                LooseRead::NotLoose => return settled(PathResolution::NotLoose, deadline),
                LooseRead::TooLarge => return settled(PathResolution::TooLarge, deadline),
                LooseRead::Malformed => return settled(PathResolution::Malformed, deadline),
            };
            // The whole tree is parsed before any entry is used, so a malformed tail cannot be
            // hidden behind an early match: a tree that does not parse establishes nothing, and
            // reporting "absent" from a half-read one would be a claim about a tree nobody read.
            let Some(entries) = parse_tree(&body) else {
                return settled(PathResolution::Malformed, deadline);
            };
            let Some(entry) = entries
                .iter()
                .find(|entry| entry.name == component.as_bytes())
            else {
                return settled(PathResolution::Absent, deadline);
            };
            if !last {
                if !entry.mode.is_directory() {
                    // A path continues through something that is not a directory, so the path
                    // itself is not in this tree. That is an absence, not a wrong kind of entry:
                    // the entry the declaration named is the LAST component, and it is not here.
                    return settled(PathResolution::Absent, deadline);
                }
                tree_id = HEXLOWER.encode(&entry.id);
                continue;
            }
            if !entry.mode.is_regular_file() {
                return settled(PathResolution::NotARegularFile, deadline);
            }
            let blob_id = HEXLOWER.encode(&entry.id);
            return settled(
                match self.read_loose_object(&blob_id, maximum, deadline)? {
                    LooseRead::Read {
                        kind: ObjectKind::Blob,
                        body,
                    } => PathResolution::Blob { body },
                    LooseRead::Read { .. } => PathResolution::Malformed,
                    LooseRead::NotLoose => PathResolution::NotLoose,
                    LooseRead::TooLarge => PathResolution::TooLarge,
                    LooseRead::Malformed => PathResolution::Malformed,
                },
                deadline,
            );
        }
        // `components` is non-empty and the loop returns on its last iteration.
        settled(PathResolution::Malformed, deadline)
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
        let loose_present = match open_directory_at(&self.objects, &revision.as_bytes()[..2]) {
            Ok(Some(prefix)) => match open_regular_at(&prefix, &revision.as_bytes()[2..]) {
                Ok(Some(_object)) => true,
                Ok(None) => false,
                Err(_) => return Err(ObjectStoreError::NotARepository),
            },
            Ok(None) => false,
            Err(_) => return Err(ObjectStoreError::NotARepository),
        };
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
        let pack_directory = match open_directory_at(&self.objects, b"pack") {
            Ok(Some(directory)) => directory,
            // No pack directory is normal for a young repository.
            Ok(None) => {
                if deadline() {
                    return Err(ObjectStoreError::Elapsed);
                }
                return Ok(false);
            }
            Err(_) => return Err(ObjectStoreError::UnreadableIndex),
        };
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let mut indexes: Vec<OsString> = directory_entries(&pack_directory)
            .map_err(|_| ObjectStoreError::UnreadableIndex)?
            .into_iter()
            .filter(|name| {
                Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension == "idx")
            })
            .collect();
        indexes.sort();
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        for index_name in indexes {
            if deadline() {
                return Err(ObjectStoreError::Elapsed);
            }
            let mut index = open_regular_at(&pack_directory, index_name.as_bytes())
                .map_err(|_| ObjectStoreError::UnreadableIndex)?
                .ok_or(ObjectStoreError::UnreadableIndex)?;
            let pack_name =
                matching_pack_name(&index_name).ok_or(ObjectStoreError::UnreadableIndex)?;
            let pack = open_regular_at(&pack_directory, pack_name.as_bytes())
                .map_err(|_| ObjectStoreError::UnreadableIndex)?
                .ok_or(ObjectStoreError::UnreadableIndex)?;
            if index_contains(&mut index, &pack, &raw, deadline)? {
                return Ok(true);
            }
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        Ok(false)
    }
}

/// The tree a commit names, as 40 lowercase hex characters.
///
/// A commit body begins with exactly `tree <40-hex>\n` and nothing may precede it. Searching the
/// body for a `tree` line instead would accept one appearing later -- inside a commit message, for
/// instance -- and let the message choose which tree Beacon walks. gpt1's S3: self-authentication
/// proves the bytes match the name, not that they are a well-formed commit.
fn commit_tree_id(body: &[u8]) -> Option<String> {
    let rest = body.strip_prefix(b"tree ")?;
    let id = rest.get(..40)?;
    if rest.get(40) != Some(&b'\n') || !id.iter().all(|byte| hex_value(*byte).is_some()) {
        return None;
    }
    // Lowercase only: the object name this becomes is used to open a file, and `.git/objects`
    // holds lowercase names. An uppercase spelling would be a different path for the same object.
    if id.iter().any(|byte| byte.is_ascii_uppercase()) {
        return None;
    }
    std::str::from_utf8(id).ok().map(str::to_owned)
}

/// Every entry of a tree, or nothing.
///
/// Validates what the format guarantees rather than what a happy path needs: a known mode, a
/// non-empty name free of `/` and the two directory names, a full 20-byte id with no truncated
/// trailing entry, and strict ordering with no duplicates. A tree that fails any of these is not a
/// tree with one bad row -- it is bytes that authenticated against a name and are not the object
/// that name is supposed to hold.
fn parse_tree(body: &[u8]) -> Option<Vec<TreeEntry<'_>>> {
    let mut entries: Vec<TreeEntry<'_>> = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let mode_end = rest.iter().position(|byte| *byte == b' ')?;
        let mode = EntryMode::parse(&rest[..mode_end])?;
        let after_mode = rest.get(mode_end + 1..)?;
        let name_end = after_mode.iter().position(|byte| *byte == 0)?;
        let name = after_mode.get(..name_end)?;
        if name.is_empty() || name.contains(&b'/') || name == b"." || name == b".." {
            return None;
        }
        let after_name = after_mode.get(name_end + 1..)?;
        let id: [u8; 20] = after_name.get(..20)?.try_into().ok()?;
        // Git orders tree entries by name, with a directory sorting as though its name ended in
        // `/`. Strictly increasing, so an equal neighbour is a duplicate rather than a tie.
        if let Some(previous) = entries.last()
            && tree_sort_key(previous.name, previous.mode) >= tree_sort_key(name, mode)
        {
            return None;
        }
        entries.push(TreeEntry { mode, name, id });
        rest = after_name.get(20..)?;
    }
    Some(entries)
}

/// The key Git sorts a tree entry by: the name, with `/` appended for a directory.
fn tree_sort_key(name: &[u8], mode: EntryMode) -> Vec<u8> {
    let mut key = name.to_vec();
    if mode.is_directory() {
        key.push(b'/');
    }
    key
}

/// The one way a definite outcome leaves a read or a walk.
///
/// Work that finished after the budget expired describes the store as it was before the answer was
/// due, so reporting it as a fact about the repository turns Beacon's own slowness into a claim
/// about the Operator's history. Every arm routes through here rather than each remembering to
/// check, which is what makes "consulted before the definite answer" true of all of them instead of
/// most of them: a non-author review staged a deadline that expired after the first blocking call
/// and got `NotLoose` back, because four arms returned before reaching a check.
fn settled<T>(outcome: T, deadline: &mut impl FnMut() -> bool) -> Result<T, ObjectStoreError> {
    if deadline() {
        return Err(ObjectStoreError::Elapsed);
    }
    Ok(outcome)
}

/// A Git object header's decimal size: no leading zero, no sign, no overflow.
pub(crate) fn parse_object_size(value: &[u8]) -> Option<usize> {
    if value.is_empty() || (value.len() > 1 && value[0] == b'0') {
        return None;
    }
    value.iter().try_fold(0_usize, |size, byte| {
        size.checked_mul(10)?.checked_add(usize::from(
            byte.checked_sub(b'0').filter(|digit| *digit < 10)?,
        ))
    })
}

fn open_directory_path(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let directory = options.open(path)?;
    if !directory.metadata()?.is_dir() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    Ok(directory)
}

fn open_directory_at(parent: &File, name: &[u8]) -> std::io::Result<Option<File>> {
    open_at(
        parent.as_raw_fd(),
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
    )
}

fn open_regular_at(parent: &File, name: &[u8]) -> std::io::Result<Option<File>> {
    let Some(file) = open_at(
        parent.as_raw_fd(),
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
    )?
    else {
        return Ok(None);
    };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    Ok(Some(file))
}

fn open_at(parent: RawFd, name: &[u8], flags: libc::c_int) -> std::io::Result<Option<File>> {
    let name =
        CString::new(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let descriptor = unsafe { libc::openat(parent, name.as_ptr(), flags) };
    if descriptor >= 0 {
        return Ok(Some(unsafe { File::from_raw_fd(descriptor) }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(error)
    }
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

fn directory_entries(directory: &File) -> std::io::Result<Vec<OsString>> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let raw_stream = unsafe { libc::fdopendir(duplicate) };
    if raw_stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicate);
        }
        return Err(error);
    }
    let stream = DirectoryStream(raw_stream);
    let mut entries = Vec::new();
    loop {
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(0) {
                break;
            }
            return Err(error);
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            entries.push(OsString::from_vec(name.to_vec()));
        }
    }
    Ok(entries)
}

fn matching_pack_name(index_name: &std::ffi::OsStr) -> Option<OsString> {
    let stem = index_name.as_bytes().strip_suffix(b".idx")?;
    if stem.is_empty() {
        return None;
    }
    let mut pack_name = stem.to_vec();
    pack_name.extend_from_slice(b".pack");
    Some(OsString::from_vec(pack_name))
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

/// Validate and binary-search one complete version 2 pack index for `target`.
fn index_contains(
    index: &mut File,
    pack: &File,
    target: &[u8; 20],
    deadline: &mut impl FnMut() -> bool,
) -> Result<bool, ObjectStoreError> {
    let mut data = Vec::new();
    index
        .read_to_end(&mut data)
        .map_err(|_| ObjectStoreError::UnreadableIndex)?;
    // The read above blocks for as long as the filesystem takes, and an index whose target bucket
    // is empty reaches `Ok(false)` below without entering the search loop -- so without this check
    // a read that completed after the budget became an absence claim.
    if deadline() {
        return Err(ObjectStoreError::Elapsed);
    }
    const HEADER: usize = 8;
    const FANOUT: usize = 256 * 4;
    const TRAILER: usize = 40;
    if data.len() < HEADER + FANOUT + TRAILER {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    if data.get(..4) != Some(b"\xfftOc") || read_u32(&data, 4) != Some(2) {
        // Version 1 indexes have no magic and a different layout. Refusing is
        // honest; silently reading one as version 2 would invent an answer.
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let fanout_at = |bucket: usize| read_u32(&data, HEADER + bucket * 4);
    let mut prior = 0_u32;
    for bucket in 0..256 {
        let value = fanout_at(bucket).ok_or(ObjectStoreError::UnreadableIndex)?;
        if value < prior {
            return Err(ObjectStoreError::UnreadableIndex);
        }
        prior = value;
    }
    let total = usize::try_from(prior).map_err(|_| ObjectStoreError::UnreadableIndex)?;
    let names_at = HEADER + FANOUT;
    let names_end = checked_table_end(names_at, total, 20)?;
    let crc_end = checked_table_end(names_end, total, 4)?;
    let offsets_end = checked_table_end(crc_end, total, 4)?;
    if offsets_end > data.len().saturating_sub(TRAILER) {
        return Err(ObjectStoreError::UnreadableIndex);
    }

    let mut fanout_count = 0_usize;
    for bucket in 0..256 {
        while fanout_count < total
            && data[names_at + fanout_count * 20] <= u8::try_from(bucket).unwrap_or(u8::MAX)
        {
            fanout_count += 1;
        }
        if fanout_at(bucket).and_then(|value| usize::try_from(value).ok()) != Some(fanout_count) {
            return Err(ObjectStoreError::UnreadableIndex);
        }
    }
    for index in 1..total {
        if index % 1024 == 0 && deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let previous = &data[names_at + (index - 1) * 20..names_at + index * 20];
        let current = &data[names_at + index * 20..names_at + (index + 1) * 20];
        if previous >= current {
            return Err(ObjectStoreError::UnreadableIndex);
        }
    }
    let pack_length = pack
        .metadata()
        .map_err(|_| ObjectStoreError::UnreadableIndex)?
        .len();
    if pack_length < 32 {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let pack_data_end = pack_length - 20;
    let mut object_offsets = vec![0_u64; total];
    let mut large_indexes = Vec::new();
    for (object, object_offset) in object_offsets.iter_mut().enumerate() {
        let offset =
            read_u32(&data, crc_end + object * 4).ok_or(ObjectStoreError::UnreadableIndex)?;
        if offset & 0x8000_0000 == 0 {
            let offset = u64::from(offset);
            if !(12..pack_data_end).contains(&offset) {
                return Err(ObjectStoreError::UnreadableIndex);
            }
            *object_offset = offset;
        } else {
            large_indexes.push((
                object,
                usize::try_from(offset & 0x7fff_ffff)
                    .map_err(|_| ObjectStoreError::UnreadableIndex)?,
            ));
        }
    }
    let large_offsets_end = checked_table_end(offsets_end, large_indexes.len(), 8)?;
    let trailer_end = large_offsets_end
        .checked_add(TRAILER)
        .ok_or(ObjectStoreError::UnreadableIndex)?;
    if trailer_end != data.len() {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let mut used_large_offsets = vec![false; large_indexes.len()];
    for (object, large_index) in large_indexes {
        let Some(used) = used_large_offsets.get_mut(large_index) else {
            return Err(ObjectStoreError::UnreadableIndex);
        };
        if *used {
            return Err(ObjectStoreError::UnreadableIndex);
        }
        *used = true;
        let offset = read_u64(&data, offsets_end + large_index * 8)
            .ok_or(ObjectStoreError::UnreadableIndex)?;
        if !(12..pack_data_end).contains(&offset) {
            return Err(ObjectStoreError::UnreadableIndex);
        }
        object_offsets[object] = offset;
    }
    if used_large_offsets.iter().any(|used| !used) {
        return Err(ObjectStoreError::UnreadableIndex);
    }

    let mut pack_header = [0_u8; 12];
    pack.read_exact_at(&mut pack_header, 0)
        .map_err(|_| ObjectStoreError::UnreadableIndex)?;
    let pack_version = u32::from_be_bytes(pack_header[4..8].try_into().unwrap());
    let pack_objects = u32::from_be_bytes(pack_header[8..12].try_into().unwrap());
    if &pack_header[..4] != b"PACK"
        || !matches!(pack_version, 2 | 3)
        || usize::try_from(pack_objects).ok() != Some(total)
    {
        return Err(ObjectStoreError::UnreadableIndex);
    }
    let mut pack_checksum = [0_u8; 20];
    pack.read_exact_at(&mut pack_checksum, pack_data_end)
        .map_err(|_| ObjectStoreError::UnreadableIndex)?;
    let computed_pack_checksum = sha1_file_prefix(pack, pack_data_end, deadline)?;
    if computed_pack_checksum != pack_checksum
        || data.get(large_offsets_end..large_offsets_end + 20)
            != Some(computed_pack_checksum.as_slice())
        || data.get(large_offsets_end + 20..trailer_end)
            != Some(Sha1::digest(&data[..large_offsets_end + 20]).as_slice())
    {
        return Err(ObjectStoreError::UnreadableIndex);
    }

    let mut objects_by_offset = object_offsets
        .into_iter()
        .enumerate()
        .map(|(name_index, offset)| (offset, name_index))
        .collect::<Vec<_>>();
    objects_by_offset.sort_unstable();
    for (position, (start, name_index)) in objects_by_offset.iter().copied().enumerate() {
        let end = objects_by_offset
            .get(position + 1)
            .map_or(pack_data_end, |(offset, _)| *offset);
        if start >= end {
            return Err(ObjectStoreError::UnreadableIndex);
        }
        let expected_crc =
            read_u32(&data, names_end + name_index * 4).ok_or(ObjectStoreError::UnreadableIndex)?;
        if crc32_file_range(pack, start, end, deadline)? != expected_crc {
            return Err(ObjectStoreError::UnreadableIndex);
        }
    }

    let first = target[0] as usize;
    let mut low = if first == 0 {
        0
    } else {
        usize::try_from(fanout_at(first - 1).ok_or(ObjectStoreError::UnreadableIndex)?)
            .map_err(|_| ObjectStoreError::UnreadableIndex)?
    };
    let mut high = usize::try_from(fanout_at(first).ok_or(ObjectStoreError::UnreadableIndex)?)
        .map_err(|_| ObjectStoreError::UnreadableIndex)?;
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

fn sha1_file_prefix(
    file: &File,
    end: u64,
    deadline: &mut impl FnMut() -> bool,
) -> Result<[u8; 20], ObjectStoreError> {
    let mut digest = Sha1::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut offset = 0_u64;
    while offset < end {
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let remaining = usize::try_from((end - offset).min(buffer.len() as u64))
            .map_err(|_| ObjectStoreError::UnreadableIndex)?;
        file.read_exact_at(&mut buffer[..remaining], offset)
            .map_err(|_| ObjectStoreError::UnreadableIndex)?;
        digest.update(&buffer[..remaining]);
        offset += u64::try_from(remaining).map_err(|_| ObjectStoreError::UnreadableIndex)?;
    }
    Ok(digest.finalize().into())
}

fn crc32_file_range(
    file: &File,
    start: u64,
    end: u64,
    deadline: &mut impl FnMut() -> bool,
) -> Result<u32, ObjectStoreError> {
    let mut crc = u32::MAX;
    let mut buffer = [0_u8; 64 * 1024];
    let mut offset = start;
    while offset < end {
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        let remaining = usize::try_from((end - offset).min(buffer.len() as u64))
            .map_err(|_| ObjectStoreError::UnreadableIndex)?;
        file.read_exact_at(&mut buffer[..remaining], offset)
            .map_err(|_| ObjectStoreError::UnreadableIndex)?;
        for byte in &buffer[..remaining] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        offset += u64::try_from(remaining).map_err(|_| ObjectStoreError::UnreadableIndex)?;
    }
    Ok(!crc)
}

fn checked_table_end(start: usize, rows: usize, width: usize) -> Result<usize, ObjectStoreError> {
    rows.checked_mul(width)
        .and_then(|length| start.checked_add(length))
        .ok_or(ObjectStoreError::UnreadableIndex)
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use data_encoding::HEXLOWER;
    use flate2::{Compression, write::ZlibEncoder};
    use sha1::{Digest, Sha1};
    use uuid::Uuid;

    use super::{
        EntryMode, LooseRead, MAX_PATH_COMPONENTS, ObjectKind, ObjectStore, ObjectStoreError,
        PathResolution, commit_tree_id, parse_object_size, parse_tree,
    };

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

    /// The deadline immediately after the index read must run before parsing can make any claim.
    ///
    /// A real pack keeps this fixture aligned with the descriptor-based reader. Corrupting the
    /// index magic makes the next operation deterministic: if five earlier `contains` checks pass,
    /// the sixth is the post-read check and reports elapsed; if one more check is allowed, parsing
    /// refuses the index before any later deadline can fire. This pins the guard's sequence without
    /// reintroducing a FIFO, which `O_NONBLOCK` and the regular-file check correctly reject. It
    /// proves ordering, not that a genuinely slow filesystem call can be interrupted.
    #[test]
    fn the_post_index_read_deadline_precedes_parsing() {
        let (repository, _) = commit_repository("post-index-read-deadline");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_directory = repository.path.join(".git/objects/pack");
        let index_path = fs::read_dir(&pack_directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .expect("git gc produced a pack index");
        let mut index = fs::read(&index_path).unwrap();

        // Preserve an absent target from an empty fanout bucket, even though the deliberate magic
        // corruption below makes parsing refuse before the search. This retains the old fixture's
        // zero-search shape if the parser is instrumented while diagnosing a regression.
        let empty_bucket = (0..256_usize)
            .find(|bucket| {
                let at = 8 + bucket * 4;
                let current = u32::from_be_bytes(index[at..at + 4].try_into().unwrap());
                let previous = if *bucket == 0 {
                    0
                } else {
                    u32::from_be_bytes(index[at - 4..at].try_into().unwrap())
                };
                current == previous
            })
            .expect("the small fixture has an empty fanout bucket");
        let target = format!("{empty_bucket:02x}{}", "00".repeat(19));
        index[0] ^= 1;
        fs::set_permissions(&index_path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(index_path, index).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        let mut calls = 0_usize;
        let mut after_five_calls = || {
            calls += 1;
            calls > 5
        };
        assert_eq!(
            store.contains(&target, &mut after_five_calls),
            Err(ObjectStoreError::Elapsed)
        );

        let mut calls = 0_usize;
        let mut after_six_calls = || {
            calls += 1;
            calls > 6
        };
        assert_eq!(
            store.contains(&target, &mut after_six_calls),
            Err(ObjectStoreError::UnreadableIndex)
        );
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

    // ---------------------------------------------------------------------------------------
    // The loose-object reader
    //
    // Four outcomes that must stay apart -- read, not-loose, too-large, malformed -- plus the
    // deadline, which is an error rather than an outcome because it is not an answer about the
    // store at all.
    // ---------------------------------------------------------------------------------------

    /// The path a loose object occupies, so a test can corrupt exactly one file.
    fn loose_path(repository: &Path, id: &str) -> PathBuf {
        repository
            .join(".git/objects")
            .join(&id[..2])
            .join(&id[2..])
    }

    /// Rewrite a loose object's file, working around the read-only mode Git gives it.
    ///
    /// The mode is set explicitly rather than through `set_readonly(false)`, which clippy refuses
    /// because it grants write to group and other as a side effect.
    fn rewrite_loose(path: &Path, bytes: &[u8]) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_loose_commit_is_read_and_names_its_own_type() {
        // The control the refusals below are measured against: without it, a reader that refused
        // everything would satisfy every other test in this section.
        let (repository, revision) = commit_repository("loose-read");
        let store = ObjectStore::open(&repository.path).unwrap();

        let read = store
            .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
            .unwrap();

        let LooseRead::Read { kind, body } = read else {
            panic!("a freshly committed object is loose and readable: {read:?}");
        };
        assert_eq!(kind, ObjectKind::Commit);
        // The body is the object WITHOUT its header, and a commit body starts with its tree line.
        assert!(
            body.starts_with(b"tree "),
            "{}",
            String::from_utf8_lossy(&body[..body.len().min(64)])
        );
        assert_eq!(body[45], b'\n');
    }

    #[test]
    fn every_object_type_is_read_as_the_type_it_is() {
        // Two mutants survived the first version of this suite: parsing `tree` as `Blob`, and
        // parsing `tag` as `Commit`. Nothing read anything but a commit, so the closed vocabulary
        // was a claim rather than a guarded property -- and the first of those mutants is directly
        // dangerous to a walk, which decides what to do next from the kind it was handed.
        //
        // Positive controls for all four, on real objects git wrote.
        let (repository, revision) = walk_repository("kinds");
        git(&repository.path, &["tag", "-a", "v1", "-m", "annotated"]);
        let store = ObjectStore::open(&repository.path).unwrap();

        let kind_of = |id: &str| match store
            .read_loose_object(id, 1_048_576, &mut never_elapsed())
            .unwrap()
        {
            LooseRead::Read { kind, .. } => kind,
            other => panic!("{id} was not readable: {other:?}"),
        };

        assert_eq!(kind_of(&revision), ObjectKind::Commit);

        let LooseRead::Read { body, .. } = store
            .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
            .unwrap()
        else {
            panic!("the commit is loose");
        };
        let tree_id = commit_tree_id(&body).unwrap();
        assert_eq!(kind_of(&tree_id), ObjectKind::Tree);

        let LooseRead::Read { body: tree, .. } = store
            .read_loose_object(&tree_id, 1_048_576, &mut never_elapsed())
            .unwrap()
        else {
            panic!("the root tree is loose");
        };
        let blob_id = HEXLOWER.encode(
            &parse_tree(&tree)
                .unwrap()
                .iter()
                .find(|entry| entry.name == b"run.sh")
                .expect("run.sh is in the root tree")
                .id,
        );
        assert_eq!(kind_of(&blob_id), ObjectKind::Blob);

        let tag_id = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "v1"])
                .current_dir(&repository.path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        assert_ne!(tag_id, revision, "an annotated tag is its own object");
        assert_eq!(kind_of(&tag_id), ObjectKind::Tag);
    }

    #[test]
    fn an_object_that_is_not_loose_is_not_reported_as_absent_or_broken() {
        // The wrong answer this kills is the one gpt1's S1 names: treating a missing loose file as
        // proof the object is packed, or as proof it is gone. This read establishes neither. The
        // object here genuinely does not exist, and the outcome is STILL only `NotLoose` -- which
        // is the point, because the reader cannot tell that case from a packed one.
        let (repository, _) = commit_repository("loose-absent");
        let store = ObjectStore::open(&repository.path).unwrap();

        let read = store
            .read_loose_object(&"a".repeat(40), 1_048_576, &mut never_elapsed())
            .unwrap();

        assert_eq!(read, LooseRead::NotLoose);
    }

    #[test]
    fn a_packed_object_is_not_loose_and_the_reader_says_only_that() {
        // Packed and absent produce the same answer from this reader, deliberately. Whichever
        // caller needs to tell them apart asks `contains`, which authenticates the pack index --
        // and here it does, so the same object is simultaneously "present" and "not loose".
        let (repository, revision) = commit_repository("loose-packed");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::NotLoose
        );
        assert!(store.contains(&revision, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn an_object_larger_than_the_callers_bound_is_a_refusal_not_a_broken_object() {
        // gpt1's S2. A legitimate file over the bound must not be reported as corrupt: that would
        // describe Beacon's own limit as the Operator's repository being broken. The same object,
        // read with a bound that fits, is `Read` -- which is what makes this a statement about the
        // bound rather than about the file.
        let (repository, revision) = commit_repository("loose-large");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&revision, 8, &mut never_elapsed())
                .unwrap(),
            LooseRead::TooLarge
        );
        assert!(matches!(
            store
                .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Read { .. }
        ));
    }

    #[test]
    fn an_object_that_inflates_past_the_bound_is_a_refusal_not_a_broken_object() {
        // The compressed file is small and the inflated object is not, which is the shape a zip
        // bomb has. The bound is enforced on the inflated size through the output capacity, so the
        // stream never finishes -- and the reader must still call that a refusal, not corruption.
        let (repository, _) = commit_repository("loose-inflates");
        let body = vec![b'x'; 4096];
        let mut object = format!("blob {}\0", body.len()).into_bytes();
        object.extend_from_slice(&body);
        let id = HEXLOWER.encode(&Sha1::digest(&object));
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&object).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 512, "{}", compressed.len());
        let path = loose_path(&repository.path, &id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &compressed).unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&id, 1024, &mut never_elapsed())
                .unwrap(),
            LooseRead::TooLarge
        );
        assert!(matches!(
            store
                .read_loose_object(&id, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Read {
                kind: ObjectKind::Blob,
                ..
            }
        ));
    }

    #[test]
    fn an_object_that_does_not_hash_to_its_own_name_is_refused() {
        // The self-authentication is the whole reason a caller may trust these bytes. Without it,
        // anything that can write into `.git/objects` chooses what Beacon compares against.
        let (repository, revision) = commit_repository("loose-tampered");
        let mut body = b"blob 5\0".to_vec();
        body.extend_from_slice(b"lied!");
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&body).unwrap();
        let forged = encoder.finish().unwrap();
        rewrite_loose(&loose_path(&repository.path, &revision), &forged);
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Malformed
        );
    }

    #[test]
    fn bytes_after_the_stream_and_a_header_that_lies_are_both_malformed() {
        // Two ways a file can hold a real object and still not BE one: a valid stream with bytes
        // appended after it, and a header whose declared size disagrees with the body it precedes.
        // Both would pass a reader that stopped at "it inflated".
        let (repository, revision) = commit_repository("loose-trailing");
        let store = ObjectStore::open(&repository.path).unwrap();
        let path = loose_path(&repository.path, &revision);
        let original = fs::read(&path).unwrap();

        let mut trailing = original.clone();
        trailing.push(0x00);
        rewrite_loose(&path, &trailing);
        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Malformed
        );

        // A header whose size is wrong, re-named so it still authenticates: the SHA-1 check
        // passes and only the size comparison refuses it.
        let mut object = b"blob 99\0".to_vec();
        object.extend_from_slice(b"short");
        let id = HEXLOWER.encode(&Sha1::digest(&object));
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&object).unwrap();
        let compressed = encoder.finish().unwrap();
        let lying = loose_path(&repository.path, &id);
        fs::create_dir_all(lying.parent().unwrap()).unwrap();
        fs::write(&lying, &compressed).unwrap();
        assert_eq!(
            store
                .read_loose_object(&id, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Malformed
        );
    }

    #[test]
    fn an_unknown_object_type_is_refused_rather_than_carried() {
        // The vocabulary is closed. An object naming a type this reader does not know is refused
        // at the reader, so no caller has to decide what an unnamed type means.
        let (repository, _) = commit_repository("loose-unknown-type");
        let mut object = b"widget 5\0".to_vec();
        object.extend_from_slice(b"stuff");
        let id = HEXLOWER.encode(&Sha1::digest(&object));
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&object).unwrap();
        let compressed = encoder.finish().unwrap();
        let path = loose_path(&repository.path, &id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &compressed).unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&id, 1_048_576, &mut never_elapsed())
                .unwrap(),
            LooseRead::Malformed
        );
    }

    #[test]
    fn an_elapsed_deadline_is_an_error_and_never_one_of_the_four_outcomes() {
        // Not evaluated is not an answer about the store. A reader that returned `NotLoose` when
        // it simply ran out of time would report Beacon's slowness as the Operator's repository
        // missing an object -- the distinction AGENTS.md requires, at this seam.
        let (repository, revision) = commit_repository("loose-deadline");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut || true)
                .unwrap_err(),
            ObjectStoreError::Elapsed
        );
    }

    /// A deadline that is false for `allowed` checks and true afterwards.
    ///
    /// `|| true` only proves the entry-time check. This is what proves the rest: it lets a read
    /// begin, expires partway through, and demands that whatever the read had already worked out is
    /// not published as a fact about the store.
    fn elapses_after(allowed: usize) -> impl FnMut() -> bool {
        let mut seen = 0_usize;
        move || {
            seen += 1;
            seen > allowed
        }
    }

    #[test]
    fn a_deadline_that_expires_mid_read_is_not_published_as_an_outcome() {
        // A non-author review staged exactly this and got `Ok(NotLoose)` where `Err(Elapsed)` was
        // required: four arms returned before reaching a check, so absent and not-evaluated were
        // separate types but not separate runtime states. Every count from 1 upward is exercised,
        // so no arm can quietly return early again without this going red.
        let (repository, revision) = walk_repository("deadline-staged");
        let store = ObjectStore::open(&repository.path).unwrap();

        // An object that is not loose: the arm the review reached.
        for allowed in 0..6 {
            let outcome =
                store.read_loose_object(&"b".repeat(40), 1_048_576, &mut elapses_after(allowed));
            assert!(
                matches!(
                    outcome,
                    Err(ObjectStoreError::Elapsed) | Ok(LooseRead::NotLoose)
                ),
                "allowed={allowed}: {outcome:?}"
            );
            if allowed < 2 {
                assert_eq!(
                    outcome,
                    Err(ObjectStoreError::Elapsed),
                    "allowed={allowed}: an unfinished read answered about the store"
                );
            }
        }

        // A real object, over its bound: `TooLarge` must not outrun the budget either.
        assert_eq!(
            store
                .read_loose_object(&revision, 8, &mut elapses_after(2))
                .unwrap_err(),
            ObjectStoreError::Elapsed
        );

        // And the successful read: expiring on the very last check still refuses to answer.
        let mut counted = 0_usize;
        let _ = store.read_loose_object(&revision, 1_048_576, &mut || {
            counted += 1;
            false
        });
        assert!(
            counted >= 2,
            "the read consulted the deadline {counted} times"
        );
        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut elapses_after(counted - 1))
                .unwrap_err(),
            ObjectStoreError::Elapsed,
            "a read that finished after its budget still returned an answer"
        );
    }

    #[test]
    fn a_walk_whose_deadline_expires_mid_traversal_publishes_nothing() {
        // The same rule one layer up: a walk that ran out of budget after reading the commit must
        // not report the path as absent, or as anything else about the repository.
        let (repository, revision) = walk_repository("deadline-walk");
        let store = ObjectStore::open(&repository.path).unwrap();
        let mut counted = 0_usize;
        let _ = store.resolve_path_at(&revision, "src/lib.rs", 1_048_576, &mut || {
            counted += 1;
            false
        });
        assert!(
            counted > 4,
            "the walk consulted the deadline {counted} times"
        );

        for allowed in 0..counted {
            assert_eq!(
                store
                    .resolve_path_at(
                        &revision,
                        "src/lib.rs",
                        1_048_576,
                        &mut elapses_after(allowed)
                    )
                    .unwrap_err(),
                ObjectStoreError::Elapsed,
                "allowed={allowed}: an unfinished walk answered about the repository"
            );
        }
    }

    #[test]
    fn a_symlinked_loose_object_is_refused_rather_than_followed() {
        // `contains` already refuses this; the reader must too, and for the same reason. A path
        // that is a symlink names a file the store does not own.
        let (repository, revision) = commit_repository("loose-read-symlink");
        let path = loose_path(&repository.path, &revision);
        let held = repository.path.join("held-object");
        fs::rename(&path, &held).unwrap();
        symlink(&held, &path).unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
                .unwrap_err(),
            ObjectStoreError::NotARepository
        );
    }

    #[test]
    fn a_name_that_is_not_forty_hex_characters_is_refused_before_any_read() {
        let (repository, _) = commit_repository("loose-bad-name");
        let store = ObjectStore::open(&repository.path).unwrap();

        for name in ["", "abc", &"z".repeat(40), &"A".repeat(40), &"a".repeat(41)] {
            assert_eq!(
                store
                    .read_loose_object(name, 1_048_576, &mut never_elapsed())
                    .unwrap_err(),
                ObjectStoreError::NotARepository,
                "{name:?} was not refused"
            );
        }
    }

    #[test]
    fn the_object_size_parser_refuses_padding_signs_and_overflow() {
        assert_eq!(parse_object_size(b"0"), Some(0));
        assert_eq!(parse_object_size(b"12"), Some(12));
        for refused in [
            &b""[..],
            b"01",
            b"007",
            b"-1",
            b"+1",
            b"1 ",
            b" 1",
            b"1a",
            b"99999999999999999999999999",
        ] {
            assert_eq!(parse_object_size(refused), None, "{refused:?} was accepted");
        }
    }

    // ---------------------------------------------------------------------------------------
    // The path walk
    // ---------------------------------------------------------------------------------------

    /// A repository with a nested file, a symlink, and an executable, committed together.
    fn walk_repository(label: &str) -> (TempDir, String) {
        let directory = TempDir::new(label);
        git(&directory.path, &["init", "--quiet"]);
        fs::create_dir_all(directory.path.join("src/deep")).unwrap();
        fs::write(directory.path.join("src/lib.rs"), "fn main() {}\n").unwrap();
        fs::write(directory.path.join("src/deep/inner.txt"), "nested\n").unwrap();
        fs::write(directory.path.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            directory.path.join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        symlink("src/lib.rs", directory.path.join("link.rs")).unwrap();
        git(&directory.path, &["add", "-A"]);
        git(&directory.path, &["commit", "--quiet", "-m", "walk"]);
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&directory.path)
            .output()
            .unwrap();
        let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        (directory, revision)
    }

    fn resolve(store: &ObjectStore, revision: &str, path: &str) -> PathResolution {
        store
            .resolve_path_at(revision, path, 1_048_576, &mut never_elapsed())
            .unwrap()
    }

    #[test]
    fn a_nested_path_resolves_to_the_exact_file_bytes() {
        // The control. Without it, a walk that answered `Absent` for everything would satisfy every
        // refusal test below. It also pins that the body is the FILE's bytes, header stripped --
        // comparing against the object's bytes instead would fail every declared entry.
        let (repository, revision) = walk_repository("walk-hit");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            resolve(&store, &revision, "src/lib.rs"),
            PathResolution::Blob {
                body: b"fn main() {}\n".to_vec()
            }
        );
        assert_eq!(
            resolve(&store, &revision, "src/deep/inner.txt"),
            PathResolution::Blob {
                body: b"nested\n".to_vec()
            }
        );
        // An executable is still a regular file, and refusing it would refuse a legitimate entry.
        assert_eq!(
            resolve(&store, &revision, "run.sh"),
            PathResolution::Blob {
                body: b"#!/bin/sh\n".to_vec()
            }
        );
    }

    #[test]
    fn a_directory_entry_is_matched_on_its_raw_mode_not_its_printed_one() {
        // `git cat-file -p` prints a directory as `040000`; the bytes are `40000`. A parser written
        // from the display refuses every directory that exists, so every nested path would come
        // back `Absent` -- a wrong answer that looks like a true one.
        assert_eq!(EntryMode::parse(b"40000"), Some(EntryMode::Directory));
        assert_eq!(EntryMode::parse(b"040000"), None);
        let (repository, revision) = walk_repository("walk-mode");
        let store = ObjectStore::open(&repository.path).unwrap();
        assert!(matches!(
            resolve(&store, &revision, "src/deep/inner.txt"),
            PathResolution::Blob { .. }
        ));
    }

    #[test]
    fn a_path_not_in_the_tree_is_absent_and_a_path_through_a_file_is_too() {
        // Absent is a claim about the repository, so it is only earned when every tree on the way
        // was read in full. Both cases here are that: the trees parsed and the name is not there.
        let (repository, revision) = walk_repository("walk-absent");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            resolve(&store, &revision, "missing.txt"),
            PathResolution::Absent
        );
        assert_eq!(
            resolve(&store, &revision, "src/missing.rs"),
            PathResolution::Absent
        );
        // `src/lib.rs` is a file, so nothing below it exists.
        assert_eq!(
            resolve(&store, &revision, "src/lib.rs/inner"),
            PathResolution::Absent
        );
    }

    #[test]
    fn a_symlink_is_not_followed_and_is_not_reported_as_a_file() {
        // The wrong answer is following the link and hashing the target's bytes, which would invent
        // agreement with a declaration about a file the path never held. gpt1's S4: the mode does
        // not establish which bytes anyone presented either, so this is its own outcome and not a
        // mismatch decided here.
        let (repository, revision) = walk_repository("walk-symlink");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            resolve(&store, &revision, "link.rs"),
            PathResolution::NotARegularFile
        );
        // A directory named where a file was expected is the same answer, for the same reason.
        assert_eq!(
            resolve(&store, &revision, "src"),
            PathResolution::NotARegularFile
        );
    }

    #[test]
    fn a_packed_repository_yields_not_loose_rather_than_absent() {
        // After a repack every object on the path is packed. The walk must carry that up as a limit
        // of this reader, never as a statement that the file was not in the revision.
        let (repository, revision) = walk_repository("walk-packed");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            resolve(&store, &revision, "src/lib.rs"),
            PathResolution::NotLoose
        );
        // And the revision is still present -- so "not loose" is provably not "not there".
        assert!(store.contains(&revision, &mut never_elapsed()).unwrap());
    }

    #[test]
    fn an_object_missing_mid_walk_is_not_loose_rather_than_absent() {
        // The packed test above stops at the COMMIT, so it never exercises the tree and blob arms
        // of the walk -- a mutant that reported those as `Absent` survived it. Here the commit is
        // loose and an object further along is not, which is the only way to reach them.
        //
        // The wrong answer is the one that matters most in this whole slice: reporting "the file
        // was not in that revision" when the truth is "Beacon could not read the object that would
        // have said". One is a claim about the Operator's history; the other is about Beacon.
        let (repository, revision) = walk_repository("walk-missing-mid");
        let store = ObjectStore::open(&repository.path).unwrap();
        let LooseRead::Read { body, .. } = store
            .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
            .unwrap()
        else {
            panic!("the commit is loose");
        };
        let tree_id = commit_tree_id(&body).unwrap();

        // The blob's own object, removed: the walk reads every tree and then cannot read the file.
        let LooseRead::Read { body: tree, .. } = store
            .read_loose_object(&tree_id, 1_048_576, &mut never_elapsed())
            .unwrap()
        else {
            panic!("the root tree is loose");
        };
        let entries = parse_tree(&tree).unwrap();
        let blob_id = HEXLOWER.encode(
            &entries
                .iter()
                .find(|entry| entry.name == b"run.sh")
                .expect("run.sh is in the root tree")
                .id,
        );
        fs::remove_file(loose_path(&repository.path, &blob_id)).unwrap();
        assert_eq!(
            resolve(&store, &revision, "run.sh"),
            PathResolution::NotLoose
        );

        // And a TREE removed mid-walk, which is the arm the surviving mutant sat in.
        fs::remove_file(loose_path(&repository.path, &tree_id)).unwrap();
        assert_eq!(
            resolve(&store, &revision, "src/lib.rs"),
            PathResolution::NotLoose
        );
    }

    #[test]
    fn a_bound_that_excludes_an_object_on_the_path_is_a_refusal_not_an_absence() {
        let (repository, revision) = walk_repository("walk-bound");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .resolve_path_at(&revision, "src/lib.rs", 8, &mut never_elapsed())
                .unwrap(),
            PathResolution::TooLarge
        );
    }

    #[test]
    fn a_commit_whose_tree_line_is_not_first_is_refused() {
        // Self-authentication proves the bytes match the name, not that they are a commit. A body
        // searched for a `tree` line anywhere would let a commit MESSAGE choose the tree walked.
        assert_eq!(
            commit_tree_id(b"tree 0123456789abcdef0123456789abcdef01234567\nparent x"),
            Some("0123456789abcdef0123456789abcdef01234567".to_owned())
        );
        for refused in [
            &b"parent x\ntree 0123456789abcdef0123456789abcdef01234567\n"[..],
            b"tree 0123456789abcdef0123456789abcdef01234567",
            b"tree 0123456789abcdef0123456789abcdef0123456\n",
            b"tree 0123456789ABCDEF0123456789abcdef01234567\n",
            b"tree zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz\n",
            b"",
        ] {
            assert_eq!(commit_tree_id(refused), None, "{refused:?} was accepted");
        }
    }

    #[test]
    fn a_tree_that_is_truncated_unordered_or_wrongly_moded_is_refused() {
        // Each of these authenticates against some name and is still not a tree. A walk that read
        // entries until one matched would accept every one of them.
        let entry = |mode: &[u8], name: &[u8]| {
            let mut out = mode.to_vec();
            out.push(b' ');
            out.extend_from_slice(name);
            out.push(0);
            out.extend_from_slice(&[0x11; 20]);
            out
        };
        let mut ordered = entry(b"100644", b"a.txt");
        ordered.extend_from_slice(&entry(b"100644", b"b.txt"));
        assert_eq!(parse_tree(&ordered).unwrap().len(), 2);

        let mut unordered = entry(b"100644", b"b.txt");
        unordered.extend_from_slice(&entry(b"100644", b"a.txt"));
        assert!(parse_tree(&unordered).is_none(), "unordered tree accepted");

        let mut duplicated = entry(b"100644", b"a.txt");
        duplicated.extend_from_slice(&entry(b"100644", b"a.txt"));
        assert!(parse_tree(&duplicated).is_none(), "duplicate name accepted");

        let mut truncated = entry(b"100644", b"a.txt");
        truncated.truncate(truncated.len() - 1);
        assert!(parse_tree(&truncated).is_none(), "truncated entry accepted");

        assert!(
            parse_tree(&entry(b"100664", b"a.txt")).is_none(),
            "unknown mode accepted"
        );
        for name in [&b""[..], b".", b"..", b"a/b"] {
            assert!(
                parse_tree(&entry(b"100644", name)).is_none(),
                "{name:?} accepted as a tree entry name"
            );
        }
    }

    #[test]
    fn a_directory_sorts_as_though_its_name_ended_in_a_slash() {
        // Git's ordering rule, and the reason a naive byte sort refuses real trees: `src` as a
        // directory sorts after `src.rs`, because the key is `src/`. Refusing that would call a
        // perfectly ordinary repository malformed.
        let entry = |mode: &[u8], name: &[u8]| {
            let mut out = mode.to_vec();
            out.push(b' ');
            out.extend_from_slice(name);
            out.push(0);
            out.extend_from_slice(&[0x22; 20]);
            out
        };
        let mut real = entry(b"100644", b"src.rs");
        real.extend_from_slice(&entry(b"40000", b"src"));
        assert_eq!(
            parse_tree(&real).unwrap().len(),
            2,
            "a real ordering was refused"
        );

        // And the reverse order, which git never writes, is refused.
        let mut wrong = entry(b"40000", b"src");
        wrong.extend_from_slice(&entry(b"100644", b"src.rs"));
        assert!(
            parse_tree(&wrong).is_none(),
            "a reversed ordering was accepted"
        );
    }

    #[test]
    fn a_tampered_tree_on_the_path_is_refused_by_its_own_name() {
        // The self-authentication is what stops anything that can write into `.git/objects` from
        // choosing which blob a path resolves to.
        let (repository, revision) = walk_repository("walk-tampered-tree");
        let store = ObjectStore::open(&repository.path).unwrap();
        let LooseRead::Read { body, .. } = store
            .read_loose_object(&revision, 1_048_576, &mut never_elapsed())
            .unwrap()
        else {
            panic!("the commit is loose");
        };
        let tree_id = commit_tree_id(&body).unwrap();
        let mut forged = b"tree 0\0".to_vec();
        forged.extend_from_slice(b"");
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&forged).unwrap();
        rewrite_loose(
            &loose_path(&repository.path, &tree_id),
            &encoder.finish().unwrap(),
        );

        assert_eq!(
            resolve(&store, &revision, "src/lib.rs"),
            PathResolution::Malformed
        );
    }

    #[test]
    fn a_malformed_or_over_deep_path_is_refused_before_the_walk_trusts_it() {
        // The declared path is validated by its own module; this re-checks the shape it depends on,
        // because a component that arrived malformed would be compared against tree entries as if
        // it were a name.
        let (repository, revision) = walk_repository("walk-path-shape");
        let store = ObjectStore::open(&repository.path).unwrap();

        for path in ["", "src//lib.rs", "./lib.rs", "src/../lib.rs", "a\0b"] {
            assert_eq!(
                resolve(&store, &revision, path),
                PathResolution::Malformed,
                "{path:?} was walked"
            );
        }
        let deep = vec!["a"; MAX_PATH_COMPONENTS + 1].join("/");
        assert_eq!(resolve(&store, &revision, &deep), PathResolution::Malformed);
    }

    #[test]
    fn the_walk_stops_at_its_deadline_rather_than_answering() {
        let (repository, revision) = walk_repository("walk-deadline");
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .resolve_path_at(&revision, "src/lib.rs", 1_048_576, &mut || true)
                .unwrap_err(),
            ObjectStoreError::Elapsed
        );
    }

    #[test]
    fn a_symlink_cannot_impersonate_a_loose_object() {
        let (repository, revision) = commit_repository("loose-symlink");
        let loose = repository
            .path
            .join(".git/objects")
            .join(&revision[..2])
            .join(&revision[2..]);
        let held = repository.path.join("held-object");
        fs::rename(&loose, &held).unwrap();
        symlink(&held, &loose).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::NotARepository
        );
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
    fn pack_contents_must_match_the_claimed_checksum() {
        let (repository, revision) = commit_repository("forged-pack-checksum");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_directory = repository.path.join(".git/objects/pack");
        let pack = fs::read_dir(&pack_directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "pack")
            })
            .expect("gc produced a pack");
        let mut permissions = fs::metadata(&pack).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&pack, permissions).unwrap();
        let mut bytes = fs::read(&pack).unwrap();
        // Keep the PACK header, pack trailer, and the index's claimed pack
        // checksum intact. The old structural check therefore answered true
        // even though the bytes authenticated by that claim had changed.
        bytes[12] ^= 0xff;
        fs::write(&pack, bytes).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn crc_rows_must_match_packed_object_bytes() {
        let (repository, revision) = commit_repository("forged-pack-crc");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_directory = repository.path.join(".git/objects/pack");
        let index = fs::read_dir(&pack_directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .expect("gc produced a pack index");
        let mut permissions = fs::metadata(&index).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&index, permissions).unwrap();
        let mut bytes = fs::read(&index).unwrap();
        let total = u32::from_be_bytes(bytes[1028..1032].try_into().unwrap()) as usize;
        let first_crc = 8 + 256 * 4 + total * 20;
        bytes[first_crc] ^= 0xff;
        let checksum_at = bytes.len() - 20;
        let checksum = Sha1::digest(&bytes[..checksum_at]);
        bytes[checksum_at..].copy_from_slice(&checksum);
        fs::write(&index, bytes).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn pack_index_contents_must_match_their_own_checksum() {
        let (repository, revision) = commit_repository("forged-index-checksum");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_directory = repository.path.join(".git/objects/pack");
        let index = fs::read_dir(&pack_directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .expect("gc produced a pack index");
        let mut permissions = fs::metadata(&index).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&index, permissions).unwrap();
        let mut bytes = fs::read(&index).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 0xff;
        fs::write(&index, bytes).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn names_unsorted_within_one_fanout_bucket_are_refused() {
        let (repository, revision) = commit_repository("unsorted-pack-names");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_directory = repository.path.join(".git/objects/pack");
        let index = fs::read_dir(&pack_directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .expect("gc produced a pack index");
        let mut permissions = fs::metadata(&index).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&index, permissions).unwrap();
        let mut bytes = fs::read(&index).unwrap();
        let total = u32::from_be_bytes(bytes[1028..1032].try_into().unwrap()) as usize;
        assert!(total >= 2, "fixture needs two packed objects");
        assert!(total <= 256, "controlled one-byte suffix must stay unique");

        for bucket in 0..256 {
            let count = if bucket < 0x42 { 0 } else { total as u32 };
            let at = 8 + bucket * 4;
            bytes[at..at + 4].copy_from_slice(&count.to_be_bytes());
        }
        let names_at = 8 + 256 * 4;
        for row in 0..total {
            let at = names_at + row * 20;
            bytes[at..at + 20].fill(0);
            bytes[at] = 0x42;
            bytes[at + 19] = u8::try_from(row).unwrap();
        }
        let first = bytes[names_at..names_at + 20].to_vec();
        let second = bytes[names_at + 20..names_at + 40].to_vec();
        bytes[names_at..names_at + 20].copy_from_slice(&second);
        bytes[names_at + 20..names_at + 40].copy_from_slice(&first);
        let checksum_at = bytes.len() - 20;
        let checksum = Sha1::digest(&bytes[..checksum_at]);
        bytes[checksum_at..].copy_from_slice(&checksum);
        fs::write(&index, bytes).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn a_pack_index_missing_crc_offsets_and_trailer_is_refused() {
        let (repository, revision) = commit_repository("truncated-index-layout");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack = repository.path.join(".git/objects/pack");
        let index = fs::read_dir(&pack)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
            .unwrap();
        let bytes = fs::read(&index).unwrap();
        let total = u32::from_be_bytes(bytes[1028..1032].try_into().unwrap()) as usize;
        let names_end = 8 + 256 * 4 + total * 20;
        let mut permissions = fs::metadata(&index).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&index, permissions).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&index)
            .unwrap()
            .set_len(u64::try_from(names_end).unwrap())
            .unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn a_pack_index_without_its_matching_pack_is_refused() {
        let (repository, revision) = commit_repository("missing-matching-pack");
        git(&repository.path, &["gc", "--quiet", "--prune=now"]);
        let pack_dir = repository.path.join(".git/objects/pack");
        let pack = fs::read_dir(&pack_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "pack")
            })
            .unwrap();
        fs::rename(&pack, pack.with_extension("pack-held")).unwrap();

        let store = ObjectStore::open(&repository.path).unwrap();
        assert_eq!(
            store.contains(&revision, &mut never_elapsed()).unwrap_err(),
            ObjectStoreError::UnreadableIndex
        );
    }

    #[test]
    fn an_unreadable_pack_directory_is_an_error_not_absence() {
        let (repository, _) = commit_repository("unreadable-pack-directory");
        let pack = repository.path.join(".git/objects/pack");
        fs::rename(&pack, repository.path.join(".git/objects/held-pack")).unwrap();
        fs::write(&pack, "not a directory").unwrap();
        let store = ObjectStore::open(&repository.path).unwrap();

        assert_eq!(
            store
                .contains(&"0".repeat(40), &mut never_elapsed())
                .unwrap_err(),
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
