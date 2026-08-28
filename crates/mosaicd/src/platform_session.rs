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
    for (interface, path) in [
        ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver"),
        ("org.freedesktop.ScreenSaver", "/ScreenSaver"),
    ] {
        let callback_accounts = Arc::clone(&accounts);
        let callback_status = Arc::clone(&status);
        connection.add_match::<(bool,), _>(
            MatchRule::new_signal(interface, "ActiveChanged").with_path(path),
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
    update_status(
        &status,
        MonitorKind::ScreenLock,
        PlatformMonitorState::Active,
    );
    loop {
        connection.process(Duration::from_secs(1))?;
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
    let callback_status = Arc::clone(&status);
    connection.add_match::<(bool,), _>(
        MatchRule::new_signal("org.freedesktop.login1.Manager", "PrepareForSleep")
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
    update_status(&status, MonitorKind::Suspend, PlatformMonitorState::Active);
    loop {
        connection.process(Duration::from_secs(1))?;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::handle_active_signal;

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
}
