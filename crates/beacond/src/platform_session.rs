#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::{OperatorAccountService, PlatformSessionEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformMonitorState {
    Starting,
    Active,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformSessionMonitorStatus {
    pub screen_lock: PlatformMonitorState,
    pub suspend: PlatformMonitorState,
}

#[derive(Clone)]
pub struct PlatformSessionMonitor {
    status: Arc<RwLock<PlatformSessionMonitorStatus>>,
}

impl Default for PlatformSessionMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformSessionMonitor {
    pub fn new() -> Self {
        Self {
            status: Arc::new(RwLock::new(PlatformSessionMonitorStatus {
                screen_lock: PlatformMonitorState::Starting,
                suspend: PlatformMonitorState::Starting,
            })),
        }
    }

    pub fn status(&self) -> PlatformSessionMonitorStatus {
        self.status
            .read()
            .map(|status| *status)
            .unwrap_or(PlatformSessionMonitorStatus {
                screen_lock: PlatformMonitorState::Unavailable,
                suspend: PlatformMonitorState::Unavailable,
            })
    }

    pub fn start(&self, accounts: Arc<OperatorAccountService>) {
        start_platform_threads(Arc::clone(&self.status), accounts);
    }
}

#[cfg(target_os = "linux")]
fn start_platform_threads(
    status: Arc<RwLock<PlatformSessionMonitorStatus>>,
    accounts: Arc<OperatorAccountService>,
) {
    let screen_status = Arc::clone(&status);
    let screen_accounts = Arc::clone(&accounts);
    if std::thread::Builder::new()
        .name("beacon-screen-lock-monitor".to_owned())
        .spawn(move || {
            if monitor_screen_lock(Arc::clone(&screen_status), screen_accounts).is_err() {
                update_status(
                    &screen_status,
                    MonitorKind::ScreenLock,
                    PlatformMonitorState::Unavailable,
                );
                eprintln!("beacon platform screen-lock monitor is unavailable");
            }
        })
        .is_err()
    {
        update_status(
            &status,
            MonitorKind::ScreenLock,
            PlatformMonitorState::Unavailable,
        );
    }

    let suspend_status = Arc::clone(&status);
    if std::thread::Builder::new()
        .name("beacon-suspend-monitor".to_owned())
        .spawn(move || {
            if monitor_suspend(Arc::clone(&suspend_status), accounts).is_err() {
                update_status(
                    &suspend_status,
                    MonitorKind::Suspend,
                    PlatformMonitorState::Unavailable,
                );
                eprintln!("beacon platform suspend monitor is unavailable");
            }
        })
        .is_err()
    {
        update_status(
            &status,
            MonitorKind::Suspend,
            PlatformMonitorState::Unavailable,
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn start_platform_threads(
    status: Arc<RwLock<PlatformSessionMonitorStatus>>,
    _accounts: Arc<OperatorAccountService>,
) {
    update_status(
        &status,
        MonitorKind::ScreenLock,
        PlatformMonitorState::Unavailable,
    );
    update_status(
        &status,
        MonitorKind::Suspend,
        PlatformMonitorState::Unavailable,
    );
}

#[derive(Clone, Copy)]
enum MonitorKind {
    ScreenLock,
    Suspend,
}

fn update_status(
    status: &RwLock<PlatformSessionMonitorStatus>,
    kind: MonitorKind,
    value: PlatformMonitorState,
) {
    if let Ok(mut status) = status.write() {
        match kind {
            MonitorKind::ScreenLock => status.screen_lock = value,
            MonitorKind::Suspend => status.suspend = value,
        }
    }
}

fn handle_active_signal(active: bool, lock: impl FnOnce()) {
    if active {
        lock();
    }
}

#[cfg(target_os = "linux")]
fn screen_provider_bit(name: &str) -> Option<u8> {
    match name {
        "org.gnome.ScreenSaver" => Some(1),
        "org.freedesktop.ScreenSaver" => Some(2),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn apply_screen_provider_probe(
    providers: &AtomicU8,
    status: &RwLock<PlatformSessionMonitorStatus>,
    name: &str,
    active: Option<bool>,
    lock: impl FnOnce(),
) {
    let Some(bit) = screen_provider_bit(name) else {
        return;
    };
    let providers = if active.is_some() {
        providers.fetch_or(bit, Ordering::AcqRel) | bit
    } else {
        providers.fetch_and(!bit, Ordering::AcqRel) & !bit
    };
    let state = if providers == 0 {
        PlatformMonitorState::Unavailable
    } else {
        PlatformMonitorState::Active
    };
    update_status(status, MonitorKind::ScreenLock, state);
    if active == Some(true) {
        lock();
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct ScreenProvider {
    bus_name: &'static str,
    interface: &'static str,
    path: &'static str,
}

#[cfg(target_os = "linux")]
const SCREEN_PROVIDERS: [ScreenProvider; 2] = [
    ScreenProvider {
        bus_name: "org.gnome.ScreenSaver",
        interface: "org.gnome.ScreenSaver",
        path: "/org/gnome/ScreenSaver",
    },
    ScreenProvider {
        bus_name: "org.freedesktop.ScreenSaver",
        interface: "org.freedesktop.ScreenSaver",
        path: "/ScreenSaver",
    },
];

#[cfg(target_os = "linux")]
fn probe_screen_provider(
    connection: &dbus::blocking::Connection,
    provider: ScreenProvider,
) -> Option<bool> {
    use std::time::Duration;

    let bus = connection.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_millis(250),
    );
    let (has_owner,): (bool,) = bus
        .method_call("org.freedesktop.DBus", "NameHasOwner", (provider.bus_name,))
        .ok()?;
    if !has_owner {
        return None;
    }

    connection
        .with_proxy(provider.bus_name, provider.path, Duration::from_millis(250))
        .method_call::<(bool,), _, _, _>(provider.interface, "GetActive", ())
        .ok()
        .map(|(active,)| active)
}

#[cfg(target_os = "linux")]
fn monitor_screen_lock(
    status: Arc<RwLock<PlatformSessionMonitorStatus>>,
    accounts: Arc<OperatorAccountService>,
) -> Result<(), dbus::Error> {
    use dbus::blocking::Connection;
    use dbus::message::MatchRule;
    use std::time::Duration;

    // dbus 0.9.12 documents typed blocking signal callbacks through add_match and bounded
    // dispatch through process: https://docs.rs/dbus/0.9.12/dbus/blocking/struct.Connection.html
    let connection = Connection::new_session()?;
    connection.set_signal_match_mode(true);
    let providers = AtomicU8::new(0);
    for provider in SCREEN_PROVIDERS {
        let callback_accounts = Arc::clone(&accounts);
        let callback_status = Arc::clone(&status);
        connection.add_match::<(bool,), _>(
            MatchRule::new_signal(provider.interface, "ActiveChanged")
                .with_sender(provider.bus_name)
                .with_path(provider.path),
            move |(active,), _, _| {
                handle_active_signal(active, || {
                    if callback_accounts
                        .lock_for_platform_event(PlatformSessionEvent::ScreenLocked)
                        .is_err()
                    {
                        update_status(
                            &callback_status,
                            MonitorKind::ScreenLock,
                            PlatformMonitorState::Unavailable,
                        );
                    }
                });
                true
            },
        )?;
    }
    loop {
        for provider in SCREEN_PROVIDERS {
            apply_screen_provider_probe(
                &providers,
                &status,
                provider.bus_name,
                probe_screen_provider(&connection, provider),
                || {
                    let _ = accounts.lock_for_platform_event(PlatformSessionEvent::ScreenLocked);
                },
            );
        }
        connection.process(Duration::from_millis(500))?;
    }
}

#[cfg(target_os = "linux")]
fn monitor_suspend(
    status: Arc<RwLock<PlatformSessionMonitorStatus>>,
    accounts: Arc<OperatorAccountService>,
) -> Result<(), dbus::Error> {
    use dbus::blocking::Connection;
    use dbus::message::MatchRule;
    use std::time::Duration;

    // systemd-logind emits PrepareForSleep(true) before suspend/hibernate:
    // https://www.freedesktop.org/wiki/Software/systemd/logind/
    let connection = Connection::new_system()?;
    let owner_status = Arc::clone(&status);
    connection.add_match::<(String, String, String), _>(
        MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
            .with_sender("org.freedesktop.DBus")
            .with_path("/org/freedesktop/DBus"),
        move |(name, _old_owner, new_owner), _, _| {
            if name == "org.freedesktop.login1" {
                update_status(
                    &owner_status,
                    MonitorKind::Suspend,
                    if new_owner.is_empty() {
                        PlatformMonitorState::Unavailable
                    } else {
                        PlatformMonitorState::Active
                    },
                );
            }
            true
        },
    )?;
    let callback_status = Arc::clone(&status);
    connection.add_match::<(bool,), _>(
        MatchRule::new_signal("org.freedesktop.login1.Manager", "PrepareForSleep")
            .with_sender("org.freedesktop.login1")
            .with_path("/org/freedesktop/login1"),
        move |(preparing,), _, _| {
            handle_active_signal(preparing, || {
                if accounts
                    .lock_for_platform_event(PlatformSessionEvent::PreparingForSleep)
                    .is_err()
                {
                    update_status(
                        &callback_status,
                        MonitorKind::Suspend,
                        PlatformMonitorState::Unavailable,
                    );
                }
            });
            true
        },
    )?;
    let proxy = connection.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(2),
    );
    let (has_owner,): (bool,) = proxy.method_call(
        "org.freedesktop.DBus",
        "NameHasOwner",
        ("org.freedesktop.login1",),
    )?;
    update_status(
        &status,
        MonitorKind::Suspend,
        if has_owner {
            PlatformMonitorState::Active
        } else {
            PlatformMonitorState::Unavailable
        },
    );
    loop {
        connection.process(Duration::from_secs(1))?;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::RwLock;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

    use super::{
        PlatformMonitorState, PlatformSessionMonitorStatus, apply_screen_provider_probe,
        handle_active_signal,
    };

    #[test]
    fn only_an_active_platform_signal_requests_a_lock() {
        let calls = AtomicUsize::new(0);

        handle_active_signal(false, || {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        handle_active_signal(true, || {
            calls.fetch_add(1, Ordering::Relaxed);
        });

        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn screen_monitor_rechecks_provider_state_when_availability_changes() {
        let providers = AtomicU8::new(0);
        let calls = AtomicUsize::new(0);
        let status = RwLock::new(PlatformSessionMonitorStatus {
            screen_lock: PlatformMonitorState::Starting,
            suspend: PlatformMonitorState::Starting,
        });

        apply_screen_provider_probe(
            &providers,
            &status,
            "org.example.Unrelated",
            Some(true),
            || {
                calls.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert_eq!(
            status.read().unwrap().screen_lock,
            PlatformMonitorState::Starting
        );
        apply_screen_provider_probe(
            &providers,
            &status,
            "org.gnome.ScreenSaver",
            Some(true),
            || {
                calls.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert_eq!(
            status.read().unwrap().screen_lock,
            PlatformMonitorState::Active
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        apply_screen_provider_probe(
            &providers,
            &status,
            "org.freedesktop.ScreenSaver",
            Some(false),
            || {
                calls.fetch_add(1, Ordering::Relaxed);
            },
        );
        apply_screen_provider_probe(&providers, &status, "org.gnome.ScreenSaver", None, || {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(
            status.read().unwrap().screen_lock,
            PlatformMonitorState::Active
        );
        apply_screen_provider_probe(
            &providers,
            &status,
            "org.freedesktop.ScreenSaver",
            None,
            || {
                calls.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert_eq!(
            status.read().unwrap().screen_lock,
            PlatformMonitorState::Unavailable
        );
    }
}
