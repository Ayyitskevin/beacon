//! Proving the sandbox from inside it.
//!
//! The parent builds a ruleset, forks, and the child applies it and reports
//! what it observed. There is no way to test Landlock in-process and recover
//! from it — restriction is irreversible for the thread — so the child is a
//! real process that exists to be discarded.
//!
//! # Async-signal-safety
//!
//! Everything between `fork` and `_exit` uses raw `libc` calls and no
//! allocation. A `fork` in a process that may hold locks on other threads can
//! deadlock if the child allocates or takes a lock, so the child touches
//! nothing that could.

use std::ffi::CString;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::landlock::{self, NETWORK_ABI_FLOOR, Ruleset};
use super::{ProbeObservation, SandboxCapability, SandboxUnavailableReason};

/// Exit codes above the observation range, so a setup failure can never be
/// mistaken for an observation.
const EXIT_NO_NEW_PRIVS: i32 = 64;
const EXIT_RESTRICT: i32 = 65;

const BIT_WROTE_INSIDE: i32 = 1 << 0;
const BIT_REFUSED_OUTSIDE: i32 = 1 << 1;
const BIT_READ_SYSTEM: i32 = 1 << 2;
const BIT_REFUSED_TCP: i32 = 1 << 3;

/// Build a ruleset, apply it in a child, and report what the child observed.
pub(super) fn verify(abi: u8) -> SandboxCapability {
    let scratch = std::env::temp_dir().join(format!("beacon-sandbox-probe-{}", Uuid::new_v4()));
    let inside = scratch.join("inside");
    let outside = scratch.join("outside");
    if std::fs::create_dir_all(&inside).is_err() || std::fs::create_dir_all(&outside).is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeInconclusive {
                detail: "the probe could not create its scratch directories".to_owned(),
            },
        };
    }
    let result = verify_in(abi, &inside, &outside);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn verify_in(abi: u8, inside: &Path, outside: &Path) -> SandboxCapability {
    // Only `inside` is a declared write target. `outside` exists so the child
    // has somewhere real to be refused, rather than being refused because a
    // path was missing.
    let ruleset = match Ruleset::build(abi, &[inside], true) {
        Ok(ruleset) => ruleset,
        Err(error) => {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::RulesetRefused {
                    errno: error.raw_os_error().unwrap_or(-1),
                },
            };
        }
    };
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::ProbeInconclusive {
                    detail: format!("the probe could not open a loopback listener: {error}"),
                },
            };
        }
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::ProbeInconclusive {
                    detail: format!("the probe listener had no address: {error}"),
                },
            };
        }
    };

    // Paths are converted before `fork`: `CString::new` allocates, and the
    // child must not.
    let (Ok(inside_file), Ok(outside_file), Ok(system_file)) = (
        cstring(inside.join("written-by-probe")),
        cstring(outside.join("written-by-probe")),
        CString::new("/proc/self/cmdline"),
    ) else {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeInconclusive {
                detail: "the probe could not encode its paths".to_owned(),
            },
        };
    };

    // Flush before forking: the child calls `_exit`, which does not flush, and
    // a duplicated buffer would otherwise be written twice.
    let _ = std::io::stdout().flush();

    // SAFETY: the child performs only async-signal-safe work — raw syscalls,
    // no allocation, no locks — and always terminates with `_exit`.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeInconclusive {
                detail: "fork failed".to_owned(),
            },
        };
    }
    if pid == 0 {
        // SAFETY: child side. Never returns.
        unsafe {
            run_child(
                ruleset.raw_fd(),
                &inside_file,
                &outside_file,
                &system_file,
                port,
            )
        }
    }

    let mut status: libc::c_int = 0;
    // SAFETY: `pid` is our child and `status` is a live local.
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    drop(listener);
    if waited != pid {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeInconclusive {
                detail: "the probe child could not be awaited".to_owned(),
            },
        };
    }
    if !libc::WIFEXITED(status) {
        // Killed by a signal: it never reported, so nothing was observed.
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeInconclusive {
                detail: format!(
                    "the probe child was terminated by signal {}",
                    libc::WTERMSIG(status)
                ),
            },
        };
    }
    interpret(abi, libc::WEXITSTATUS(status), ruleset.restricts_network())
}

fn interpret(abi: u8, code: i32, restricts_network: bool) -> SandboxCapability {
    match code {
        EXIT_NO_NEW_PRIVS => {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::NoNewPrivsRefused { errno: 0 },
            };
        }
        EXIT_RESTRICT => {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::RestrictRefused { errno: 0 },
            };
        }
        code if code
            > BIT_WROTE_INSIDE | BIT_REFUSED_OUTSIDE | BIT_READ_SYSTEM | BIT_REFUSED_TCP =>
        {
            return SandboxCapability::Unavailable {
                reason: SandboxUnavailableReason::ProbeInconclusive {
                    detail: format!("the probe child reported an unknown status {code}"),
                },
            };
        }
        _ => {}
    }
    let observed = ProbeObservation {
        wrote_inside_target: code & BIT_WROTE_INSIDE != 0,
        refused_write_outside_target: code & BIT_REFUSED_OUTSIDE != 0,
        read_system_path: code & BIT_READ_SYSTEM != 0,
        refused_tcp_connect: code & BIT_REFUSED_TCP != 0,
    };
    if !observed.filesystem_held() {
        // The syscalls succeeded and the containment did not hold.
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeRefused { observed },
        };
    }
    if !restricts_network {
        // Below the network floor the TCP observation carries no meaning, so
        // the capability is named for what it can actually promise.
        return SandboxCapability::FilesystemOnly {
            landlock_abi: abi,
            observed,
        };
    }
    if !observed.refused_tcp_connect {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeRefused { observed },
        };
    }
    debug_assert!(abi >= NETWORK_ABI_FLOOR);
    SandboxCapability::Enforcing {
        landlock_abi: abi,
        observed,
    }
}

fn cstring(path: PathBuf) -> Result<CString, ()> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).map_err(|_| ())
}

/// The child. Applies the ruleset, observes, and exits. Never returns.
///
/// # Safety
///
/// Runs between `fork` and `_exit`. Every call here is async-signal-safe and
/// nothing allocates.
unsafe fn run_child(
    ruleset_fd: libc::c_int,
    inside: &CString,
    outside: &CString,
    system: &CString,
    port: u16,
) -> ! {
    // SAFETY: child side, about to be discarded; restriction is meant to stick.
    if let Err(errno) = unsafe { landlock::restrict_current_thread(ruleset_fd) } {
        // Distinguish which half failed by errno being set at all.
        let _ = errno;
        // SAFETY: `_exit` is async-signal-safe and terminates immediately.
        unsafe { libc::_exit(EXIT_RESTRICT) }
    }

    let mut observed = 0;
    // SAFETY: each path is a live NUL-terminated string owned by the parent's
    // address space, which this child shares copy-on-write.
    unsafe {
        let fd = libc::open(inside.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o600);
        if fd >= 0 {
            observed |= BIT_WROTE_INSIDE;
            libc::close(fd);
        }
        let fd = libc::open(outside.as_ptr(), libc::O_WRONLY | libc::O_CREAT, 0o600);
        if fd < 0 {
            observed |= BIT_REFUSED_OUTSIDE;
        } else {
            libc::close(fd);
        }
        let fd = libc::open(system.as_ptr(), libc::O_RDONLY);
        if fd >= 0 {
            observed |= BIT_READ_SYSTEM;
            libc::close(fd);
        }
        let socket = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if socket < 0 {
            // Creating the socket is not the restriction under test; treat an
            // unavailable socket as a refusal to connect, since no connection
            // can occur.
            observed |= BIT_REFUSED_TCP;
        } else {
            let mut address: libc::sockaddr_in = std::mem::zeroed();
            address.sin_family = libc::AF_INET as libc::sa_family_t;
            address.sin_port = port.to_be();
            address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            let connected = libc::connect(
                socket,
                std::ptr::addr_of!(address).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );
            if connected != 0 {
                observed |= BIT_REFUSED_TCP;
            }
            libc::close(socket);
        }
        let _ = ruleset_fd;
        libc::_exit(observed)
    }
}
