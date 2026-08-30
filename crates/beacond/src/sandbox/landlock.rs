//! Landlock, reached directly through `libc`.
//!
//! Three syscalls and one `prctl`. No `bubblewrap`, no sandbox crate, no
//! unprivileged user namespaces — see ADR 0029 for why.
//!
//! # Two irreversible properties
//!
//! `PR_SET_NO_NEW_PRIVS` and `landlock_restrict_self` cannot be undone for the
//! calling thread, and the restriction is inherited by its children. The
//! supervisor must therefore never restrict itself. Everything in this module
//! that restricts is documented as child-side and is called only between `fork`
//! and `exec`, or inside a probe child that exists to be thrown away.

use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

// Syscall numbers are stable ABI across architectures for these three.
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1;
const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;

/// The first ABI that restricts TCP bind and connect. Below this, Landlock
/// cannot satisfy "network access is denied by default".
pub(crate) const NETWORK_ABI_FLOOR: u8 = 4;

/// Filesystem access rights, as of ABI 5. `handled_access_fs` names the rights
/// the ruleset governs; anything governed and not granted by a rule is denied.
mod fs_access {
    pub(super) const EXECUTE: u64 = 1 << 0;
    pub(super) const WRITE_FILE: u64 = 1 << 1;
    pub(super) const READ_FILE: u64 = 1 << 2;
    pub(super) const READ_DIR: u64 = 1 << 3;
    pub(super) const REMOVE_DIR: u64 = 1 << 4;
    pub(super) const REMOVE_FILE: u64 = 1 << 5;
    pub(super) const MAKE_CHAR: u64 = 1 << 6;
    pub(super) const MAKE_DIR: u64 = 1 << 7;
    pub(super) const MAKE_REG: u64 = 1 << 8;
    pub(super) const MAKE_SOCK: u64 = 1 << 9;
    pub(super) const MAKE_FIFO: u64 = 1 << 10;
    pub(super) const MAKE_BLOCK: u64 = 1 << 11;
    pub(super) const MAKE_SYM: u64 = 1 << 12;
    pub(super) const REFER: u64 = 1 << 13;
    pub(super) const TRUNCATE: u64 = 1 << 14;
}

/// TCP rights, ABI 4 and later.
const LANDLOCK_ACCESS_NET_BIND_TCP: u64 = 1 << 0;
const LANDLOCK_ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

/// Rights a Run may exercise anywhere: load a binary, read a library, list a
/// directory. Without these a sandboxed process cannot `exec` at all, so a
/// ruleset that grants nothing is not strict — it is broken.
pub(crate) const READ_RIGHTS: u64 = fs_access::EXECUTE | fs_access::READ_FILE | fs_access::READ_DIR;

/// Rights a Run may exercise only beneath a declared target.
pub(crate) const WRITE_RIGHTS: u64 = fs_access::WRITE_FILE
    | fs_access::MAKE_REG
    | fs_access::MAKE_DIR
    | fs_access::MAKE_SYM
    | fs_access::MAKE_FIFO
    | fs_access::MAKE_SOCK
    | fs_access::REMOVE_FILE
    | fs_access::REMOVE_DIR
    | fs_access::TRUNCATE
    | fs_access::REFER;

/// Every right this build governs. Rights outside this mask are unrestricted,
/// which is why the mask is stated once rather than assembled per call.
const HANDLED_FS: u64 = READ_RIGHTS | WRITE_RIGHTS | fs_access::MAKE_CHAR | fs_access::MAKE_BLOCK;

/// `struct landlock_ruleset_attr`.
#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

/// `struct landlock_path_beneath_attr`, which the kernel declares packed.
///
/// A `u64` followed by an `i32` is 12 bytes packed and 16 padded. Passing the
/// padded size makes the kernel read a size it did not expect; this is the
/// classic way to get a silently wrong ruleset, so the size is asserted below.
#[repr(C, packed)]
struct PathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

const _: () = assert!(std::mem::size_of::<PathBeneathAttr>() == 12);

/// The Landlock ABI this kernel implements, or `None` if it implements none.
pub(crate) fn abi_version() -> Option<u8> {
    // SAFETY: the version query passes a null attr with size 0, which is the
    // documented form; it creates no ruleset and owns no resource.
    let version = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    u8::try_from(version).ok().filter(|version| *version > 0)
}

/// A populated ruleset, ready to be applied by a child.
///
/// Holding this in the parent is inert: a ruleset only takes effect when a
/// thread calls `restrict_current_thread` with it.
pub(crate) struct Ruleset {
    fd: OwnedFd,
    restricts_network: bool,
}

impl Ruleset {
    /// Build a ruleset granting read and execute everywhere, and write only
    /// beneath `write_targets`.
    ///
    /// `restrict_network` is honoured only when the ABI supports it; the caller
    /// learns whether it took effect from [`Ruleset::restricts_network`] rather
    /// than assuming, because a silently unrestricted network is the failure
    /// this whole module exists to prevent.
    pub(crate) fn build(
        abi: u8,
        write_targets: &[&Path],
        restrict_network: bool,
    ) -> std::io::Result<Self> {
        let ruleset = Self::empty(abi, restrict_network)?;
        ruleset.grant(Path::new("/"), READ_RIGHTS)?;
        for target in write_targets {
            ruleset.grant(target, READ_RIGHTS | WRITE_RIGHTS)?;
        }
        Ok(ruleset)
    }

    /// Build the execution ruleset from the launcher's retained root.
    ///
    /// The borrow is used only while adding the rule. The ruleset stores the
    /// kernel's reference to that object; it neither stores nor closes the
    /// caller's descriptor.
    pub(crate) fn build_for_worktree(
        abi: u8,
        worktree_root: BorrowedFd<'_>,
        restrict_network: bool,
    ) -> std::io::Result<Self> {
        let ruleset = Self::empty(abi, restrict_network)?;
        ruleset.grant(Path::new("/"), READ_RIGHTS)?;
        ruleset.grant_fd(worktree_root, READ_RIGHTS | WRITE_RIGHTS)?;
        Ok(ruleset)
    }

    fn empty(abi: u8, restrict_network: bool) -> std::io::Result<Self> {
        let restricts_network = restrict_network && abi >= NETWORK_ABI_FLOOR;
        let attr = RulesetAttr {
            handled_access_fs: HANDLED_FS,
            handled_access_net: if restricts_network {
                LANDLOCK_ACCESS_NET_BIND_TCP | LANDLOCK_ACCESS_NET_CONNECT_TCP
            } else {
                0
            },
        };
        // SAFETY: `attr` is a live, fully initialized value of the layout the
        // kernel expects, and its size is passed from `size_of` rather than a
        // literal.
        let fd = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0usize,
            )
        };
        let fd = libc::c_int::try_from(fd).map_err(|_| std::io::Error::last_os_error())?;
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: the kernel returned a fresh descriptor and nothing else owns it.
        let ruleset = Self {
            fd: unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) },
            restricts_network,
        };
        Ok(ruleset)
    }

    fn grant(&self, path: &Path, allowed_access: u64) -> std::io::Result<()> {
        let raw = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: `raw` is a live NUL-terminated path for the duration of the call.
        let parent = unsafe { libc::open(raw.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if parent < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `open` returned a fresh descriptor owned here.
        let parent = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(parent) };
        self.grant_fd(parent.as_fd(), allowed_access)
    }

    fn grant_fd(&self, parent: BorrowedFd<'_>, allowed_access: u64) -> std::io::Result<()> {
        let attr = PathBeneathAttr {
            allowed_access,
            parent_fd: parent.as_raw_fd(),
        };
        // SAFETY: `attr` matches the kernel's packed layout, its size is taken
        // from `size_of` (asserted 12 above), and the borrowed parent remains
        // open for the duration of the syscall.
        let added = unsafe {
            libc::syscall(
                SYS_LANDLOCK_ADD_RULE,
                self.fd.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                &attr as *const PathBeneathAttr,
                0usize,
            )
        };
        if added != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Whether this ruleset governs TCP. False on an ABI below the floor.
    pub(crate) fn restricts_network(&self) -> bool {
        self.restricts_network
    }

    pub(crate) fn raw_fd(&self) -> libc::c_int {
        self.fd.as_raw_fd()
    }
}

/// Apply a ruleset to the calling thread. **Irreversible, and inherited.**
///
/// Call this only in a child that is about to `exec`, or in a probe child that
/// exists to be discarded. Calling it in the supervisor restricts the
/// supervisor permanently.
///
/// # Safety
///
/// Async-signal-safe: two syscalls and no allocation, so it is legal between
/// `fork` and `exec`.
pub(crate) unsafe fn restrict_current_thread(ruleset_fd: libc::c_int) -> Result<(), i32> {
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS takes scalars only. Landlock
    // refuses to arm without it, so it must come first.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(errno());
    }
    // SAFETY: `ruleset_fd` is a ruleset descriptor inherited across `fork`.
    if unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd, 0usize) } != 0 {
        return Err(errno());
    }
    Ok(())
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use uuid::Uuid;

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("beacon-landlock-borrowed-root-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn open_path(path: &Path) -> OwnedFd {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is live and NUL-terminated for this call.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        assert!(
            fd >= 0,
            "anchor open failed: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `open` returned a fresh descriptor owned here.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    #[test]
    fn borrowed_worktree_anchor_keeps_identity_and_remains_live() {
        let Some(abi) = abi_version() else {
            eprintln!("skipped: this host reports no Landlock ABI");
            return;
        };
        let scratch = TempDirectory::new();
        let original_name = scratch.0.join("anchor");
        let moved_name = scratch.0.join("descriptor-pinned");
        fs::create_dir(&original_name).unwrap();
        let anchor = open_path(&original_name);
        fs::rename(&original_name, &moved_name).unwrap();
        fs::create_dir(&original_name).unwrap();

        let ruleset = Ruleset::build_for_worktree(abi, anchor.as_fd(), false).unwrap();
        // The helper borrowed rather than consumed or closed the descriptor.
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `anchor` is expected to remain live and `metadata` is writable.
        assert_eq!(
            unsafe { libc::fstat(anchor.as_raw_fd(), metadata.as_mut_ptr()) },
            0
        );

        let allowed = CString::new(moved_name.join("allowed").as_os_str().as_bytes()).unwrap();
        let denied = CString::new(original_name.join("denied").as_os_str().as_bytes()).unwrap();
        // SAFETY: the child uses raw async-signal-safe syscalls only and exits.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // SAFETY: child side; restriction is intended to be irreversible.
            unsafe {
                if restrict_current_thread(ruleset.raw_fd()).is_err() {
                    libc::_exit(64);
                }
                let allowed_fd = libc::open(
                    allowed.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC,
                    0o600,
                );
                let denied_fd = libc::open(
                    denied.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC,
                    0o600,
                );
                let mut result = 0;
                if allowed_fd >= 0 {
                    result |= 1;
                    libc::close(allowed_fd);
                }
                if denied_fd < 0 {
                    result |= 2;
                } else {
                    libc::close(denied_fd);
                }
                libc::_exit(result);
            }
        }

        let mut status = 0;
        // SAFETY: `pid` is this process's child and `status` is writable.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 3);
        assert!(moved_name.join("allowed").exists());
        assert!(!original_name.join("denied").exists());
    }
}
