//! Descriptor-bound Declared Check launch — ADR 0032.
//!
//! A pathname is display evidence, not execution authority. This module opens
//! one worktree root, builds Landlock's write grant from that same descriptor,
//! and resolves every declared cwd beneath it atomically with `openat2`.

use std::ffi::CString;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::SandboxCapability;
use super::bounded::{self, ExecutionBounds, ExecutionOutcome, ExecutionRecord};
use super::landlock::Ruleset;

const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
const RESOLVE_BENEATH: u64 = 0x08;
const RESOLVE_FLAGS: u64 = RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS;
const OPEN_FLAGS: u64 = (libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC) as u64;
const ROOT_FLAGS: libc::c_int =
    libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;

/// The stable kernel ABI for `struct open_how`.
///
/// This local definition is deliberate: libc's `open_how` is non-exhaustive,
/// while Beacon must pass the exact 24-byte ABI shape it reviewed.
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const _: () = assert!(std::mem::size_of::<OpenHow>() == 24);

pub(crate) struct WorktreeLauncher {
    worktree_path: PathBuf,
    sandbox_version: String,
    state: LauncherState,
}

enum LauncherState {
    Ready { root: OwnedFd, ruleset: Ruleset },
    Refused { reason: String },
}

impl WorktreeLauncher {
    pub(crate) fn bind(worktree_path: &Path, capability: &SandboxCapability) -> Self {
        let sandbox_version = capability.receipt_version();
        let SandboxCapability::Enforcing { landlock_abi, .. } = capability else {
            return Self::refused(
                worktree_path,
                sandbox_version,
                "this host cannot contain a Run".to_owned(),
            );
        };

        let root = match open_root(worktree_path) {
            Ok(root) => root,
            Err(error) => {
                return Self::refused(
                    worktree_path,
                    sandbox_version,
                    stable_error("worktree root unavailable", &error),
                );
            }
        };
        let ruleset = match Ruleset::build_for_worktree(*landlock_abi, root.as_fd(), true) {
            Ok(ruleset) => ruleset,
            Err(error) => {
                return Self::refused(
                    worktree_path,
                    sandbox_version,
                    stable_error("worktree sandbox ruleset unavailable", &error),
                );
            }
        };

        Self {
            worktree_path: worktree_path.to_path_buf(),
            sandbox_version,
            state: LauncherState::Ready { root, ruleset },
        }
    }

    pub(crate) fn run(
        &self,
        argv: &[String],
        declared_working_directory: &str,
        bounds: ExecutionBounds,
        interrupt: Option<mpsc::Receiver<()>>,
    ) -> ExecutionRecord {
        self.run_after_binding(argv, declared_working_directory, bounds, interrupt, || {})
    }

    #[cfg(test)]
    pub(super) fn run_with_before_spawn(
        &self,
        argv: &[String],
        declared_working_directory: &str,
        bounds: ExecutionBounds,
        interrupt: Option<mpsc::Receiver<()>>,
        before_spawn: impl FnOnce(),
    ) -> ExecutionRecord {
        self.run_after_binding(
            argv,
            declared_working_directory,
            bounds,
            interrupt,
            before_spawn,
        )
    }

    fn run_after_binding(
        &self,
        argv: &[String],
        declared_working_directory: &str,
        bounds: ExecutionBounds,
        interrupt: Option<mpsc::Receiver<()>>,
        before_spawn: impl FnOnce(),
    ) -> ExecutionRecord {
        let display = self.worktree_path.join(declared_working_directory);
        let refuse = |reason| refusal(argv, &display, reason, &self.sandbox_version);
        let LauncherState::Ready { root, ruleset } = &self.state else {
            let LauncherState::Refused { reason } = &self.state else {
                unreachable!();
            };
            return refuse(reason.clone());
        };

        let directory = match open_beneath(root, declared_working_directory) {
            Ok(directory) => directory,
            Err(error) => return refuse(resolve_error(&error)),
        };

        bounded::run_bounded(
            argv,
            bounded::BoundWorkingDirectory::new(&display, directory.as_fd()),
            ruleset,
            bounds,
            &self.sandbox_version,
            interrupt,
            before_spawn,
        )
    }

    fn refused(worktree_path: &Path, sandbox_version: String, reason: String) -> Self {
        Self {
            worktree_path: worktree_path.to_path_buf(),
            sandbox_version,
            state: LauncherState::Refused { reason },
        }
    }
}

fn refusal(
    argv: &[String],
    display: &Path,
    reason: String,
    sandbox_version: &str,
) -> ExecutionRecord {
    ExecutionRecord {
        argv: argv.to_vec(),
        working_directory: display.to_string_lossy().into_owned(),
        outcome: ExecutionOutcome::NotStarted { reason },
        stdout: String::new(),
        stderr: String::new(),
        output_truncated: false,
        output_incomplete: false,
        elapsed_ms: 0,
        sandbox_version: sandbox_version.to_owned(),
    }
}

fn open_root(path: &Path) -> std::io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: `path` is a live NUL-terminated string and the flags request a
    // descriptor without dereferencing the final component.
    let fd = unsafe { libc::open(path.as_ptr(), ROOT_FLAGS) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `open` returned a fresh descriptor and this value owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_beneath(root: &OwnedFd, declared: &str) -> std::io::Result<OwnedFd> {
    use std::os::fd::AsRawFd;

    let declared = CString::new(declared.as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let how = OpenHow {
        flags: OPEN_FLAGS,
        mode: 0,
        resolve: RESOLVE_FLAGS,
    };
    // SAFETY: the root and path remain live, `how` is the exact kernel ABI
    // layout, and its reviewed size is passed explicitly.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root.as_raw_fd(),
            declared.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = i32::try_from(fd).map_err(|_| std::io::Error::from_raw_os_error(libc::EMFILE))?;
    // SAFETY: `openat2` returned a fresh descriptor and this value owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn resolve_error(error: &std::io::Error) -> String {
    let errno = error.raw_os_error().unwrap_or(-1);
    let category = match errno {
        libc::EXDEV | libc::ELOOP | libc::EAGAIN => "unsafe or raced working-directory resolution",
        libc::EACCES | libc::ENOENT | libc::ENOTDIR | libc::ENAMETOOLONG => {
            "working directory unavailable or invalid"
        }
        libc::EMFILE | libc::ENFILE | libc::ENOMEM => "supervisor resource failure",
        libc::ENOSYS | libc::EINVAL | libc::E2BIG | libc::EPERM => {
            "working-directory resolver unavailable"
        }
        libc::EBADF => "internal worktree descriptor invariant failure",
        _ => "uncategorized working-directory resolver failure",
    };
    format!("{category} (errno {errno})")
}

fn stable_error(category: &str, error: &std::io::Error) -> String {
    format!("{category} (errno {})", error.raw_os_error().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::symlink;

    use uuid::Uuid;

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("beacon-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolver_uses_the_exact_accepted_flags() {
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(OPEN_FLAGS, 0o12200000);
            assert_eq!(ROOT_FLAGS, 0o12600000);
        }
        assert_eq!(RESOLVE_FLAGS, RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS);
        assert_eq!(RESOLVE_FLAGS, 0x0a);
    }

    #[test]
    fn relative_internal_symlinks_work_but_absolute_and_magic_targets_are_refused() {
        let root = TempDirectory::new("resolver-symlinks");
        fs::create_dir(root.0.join("inside")).unwrap();
        symlink("inside", root.0.join("relative")).unwrap();
        symlink(&root.0, root.0.join("absolute-inside")).unwrap();

        let target = open_root(&root.0.join("inside")).unwrap();
        symlink(
            format!("/proc/self/fd/{}", target.as_raw_fd()),
            root.0.join("magic"),
        )
        .unwrap();
        let anchor = open_root(&root.0).unwrap();

        assert!(open_beneath(&anchor, "relative").is_ok());
        for path in ["absolute-inside", "magic"] {
            let error = open_beneath(&anchor, path).unwrap_err();
            assert!(
                matches!(error.raw_os_error(), Some(libc::EXDEV | libc::ELOOP)),
                "{path} failed with the wrong errno: {error:?}"
            );
        }
    }

    #[test]
    fn every_resolver_errno_has_a_stable_fail_closed_category() {
        let cases = [
            (libc::EXDEV, "unsafe or raced working-directory resolution"),
            (libc::ELOOP, "unsafe or raced working-directory resolution"),
            (libc::EAGAIN, "unsafe or raced working-directory resolution"),
            (libc::EACCES, "working directory unavailable or invalid"),
            (libc::ENOENT, "working directory unavailable or invalid"),
            (libc::ENOTDIR, "working directory unavailable or invalid"),
            (
                libc::ENAMETOOLONG,
                "working directory unavailable or invalid",
            ),
            (libc::EMFILE, "supervisor resource failure"),
            (libc::ENFILE, "supervisor resource failure"),
            (libc::ENOMEM, "supervisor resource failure"),
            (libc::ENOSYS, "working-directory resolver unavailable"),
            (libc::EINVAL, "working-directory resolver unavailable"),
            (libc::E2BIG, "working-directory resolver unavailable"),
            (libc::EPERM, "working-directory resolver unavailable"),
            (
                libc::EBADF,
                "internal worktree descriptor invariant failure",
            ),
            (
                libc::EIO,
                "uncategorized working-directory resolver failure",
            ),
        ];

        for (errno, category) in cases {
            assert_eq!(
                resolve_error(&std::io::Error::from_raw_os_error(errno)),
                format!("{category} (errno {errno})")
            );
        }
    }
}
