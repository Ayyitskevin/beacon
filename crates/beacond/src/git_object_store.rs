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
            return Ok(LooseRead::NotLoose);
        };
        let Some(mut file) = open_regular_at(&prefix, &object_id.as_bytes()[2..])
            .map_err(|_| ObjectStoreError::NotARepository)?
        else {
            return Ok(LooseRead::NotLoose);
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
            return Ok(LooseRead::TooLarge);
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
            Err(_) => return Ok(LooseRead::Malformed),
        };
        if status != Status::StreamEnd || decoder.total_in() != compressed.len() as u64 {
            // Either the stream did not finish -- which includes hitting the inflation bound --
            // or bytes trail it. A loose object file holds one zlib stream and nothing after it.
            return Ok(if object.len() > maximum {
                LooseRead::TooLarge
            } else {
                LooseRead::Malformed
            });
        }
        if object.len() > maximum {
            return Ok(LooseRead::TooLarge);
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }

        // The name authenticates the whole uncompressed form, header included, so this runs before
        // the header is trusted for anything.
        if Sha1::digest(&object)[..] != raw_id {
            return Ok(LooseRead::Malformed);
        }
        let Some(header_end) = object.iter().position(|byte| *byte == 0) else {
            return Ok(LooseRead::Malformed);
        };
        let header = &object[..header_end];
        let Some(type_end) = header.iter().position(|byte| *byte == b' ') else {
            return Ok(LooseRead::Malformed);
        };
        let Some(kind) = ObjectKind::parse(&header[..type_end]) else {
            return Ok(LooseRead::Malformed);
        };
        let body = &object[header_end + 1..];
        if parse_object_size(&header[type_end + 1..]) != Some(body.len()) {
            return Ok(LooseRead::Malformed);
        }
        if deadline() {
            return Err(ObjectStoreError::Elapsed);
        }
        Ok(LooseRead::Read {
            kind,
            body: body.to_vec(),
        })
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

    use super::{LooseRead, ObjectKind, ObjectStore, ObjectStoreError, parse_object_size};

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
