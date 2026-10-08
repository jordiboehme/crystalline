//! The cross-platform half of the clean stop on Windows sign-out and
//! shutdown: the message ids, the decision a window procedure makes for each
//! message, and the signal the daemon sends when its stop is done. Pure, so
//! it is tested on every OS; `session_end_windows` owns the window.
//!
//! Why a window at all (`research/2026-10-07-windows-daemon-session-end.md`):
//! a process with no window and no console is ended with the user's session
//! and is told nothing. Console control events reach services only, and
//! `FreeConsole` clears the handlers anyway. A hidden top-level window gets
//! `WM_QUERYENDSESSION` and `WM_ENDSESSION`; a message-only window would not
//! (it gets no broadcasts).

// Only the Windows window reads most of this; `notify_stopped` is the one
// item every platform calls.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub(crate) const WM_DESTROY: u32 = 0x0002;
pub(crate) const WM_CLOSE: u32 = 0x0010;
pub(crate) const WM_QUERYENDSESSION: u32 = 0x0011;
pub(crate) const WM_ENDSESSION: u32 = 0x0016;
/// `WM_APP + 1`: this process asking its own window to go.
pub(crate) const QUIT_MESSAGE: u32 = 0x8001;
pub(crate) const ENDSESSION_CLOSEAPP: u32 = 0x0000_0001;
/// A forced end. [`decide`] does not read it (a forced sign-out is still a
/// sign-out); it is named for the tests and the reader.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const ENDSESSION_CRITICAL: u32 = 0x4000_0000;
pub(crate) const ENDSESSION_LOGOFF: u32 = 0x8000_0000;

/// How long the window procedure holds `WM_ENDSESSION` while the daemon
/// stops. Returning from it is Windows' permission to end the process at
/// once, and a process without a visible window is ended 5 s after the
/// message in any case, so the wait stays under that.
pub const ENDSESSION_WAIT: Duration = Duration::from_secs(4);

/// What the window procedure does with one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndAction {
    /// `WM_QUERYENDSESSION`: answer TRUE at once and do nothing yet.
    AllowEnd,
    /// Trigger the daemon's graceful stop with `reason`; with `wait`, hold the
    /// message until the stop is done or [`ENDSESSION_WAIT`] has passed.
    Stop { reason: &'static str, wait: bool },
    /// `WM_ENDSESSION` with `wParam` FALSE: the end was cancelled.
    Ignore,
    /// [`QUIT_MESSAGE`]: destroy the window.
    Quit,
    /// `WM_DESTROY`: end the message loop.
    Destroyed,
    /// Everything else, for `DefWindowProcW`.
    PassOn,
}

/// The decision for one message. `lparam` is read as its low 32 bits, so a
/// sign-extended `ENDSESSION_LOGOFF` reads the same. Restart Manager's
/// `ENDSESSION_CLOSEAPP` is named first: an installer replacing the binary
/// is the most specific reason.
pub(crate) fn decide(message: u32, wparam: usize, lparam: isize) -> EndAction {
    let flags = lparam as u32;
    match message {
        WM_QUERYENDSESSION => EndAction::AllowEnd,
        WM_ENDSESSION if wparam == 0 => EndAction::Ignore,
        WM_ENDSESSION => {
            let reason = if flags & ENDSESSION_CLOSEAPP != 0 {
                "an installer closing it (Restart Manager)"
            } else if flags & ENDSESSION_LOGOFF != 0 {
                "Windows sign-out"
            } else {
                "Windows shutdown or restart"
            };
            EndAction::Stop { reason, wait: true }
        }
        WM_CLOSE => EndAction::Stop {
            reason: "a close request (WM_CLOSE)",
            wait: false,
        },
        QUIT_MESSAGE => EndAction::Quit,
        WM_DESTROY => EndAction::Destroyed,
        _ => EndAction::PassOn,
    }
}

static STOPPED: OnceLock<Mutex<Option<SyncSender<()>>>> = OnceLock::new();

/// The receiving end of "the stop is done", for the window. A new call
/// replaces the sender, so each window (and each test) gets its own.
pub(crate) fn stopped_receiver() -> Receiver<()> {
    let (tx, rx) = sync_channel(1);
    let slot = STOPPED.get_or_init(|| Mutex::new(None));
    *slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    rx
}

/// Say the stop is done: the record, the socket and the lock file are gone
/// and the process exits next. Once; a no-op when nobody waits (every
/// platform but Windows, a daemon without the window).
pub(crate) fn notify_stopped() {
    if let Some(slot) = STOPPED.get()
        && let Some(tx) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    {
        let _ = tx.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_end_session_never_vetoes() {
        for lparam in [
            0,
            ENDSESSION_LOGOFF as isize,
            ENDSESSION_CLOSEAPP as isize,
            ENDSESSION_CRITICAL as isize,
        ] {
            assert_eq!(
                decide(WM_QUERYENDSESSION, 0, lparam),
                EndAction::AllowEnd,
                "{lparam:#x}"
            );
        }
    }

    #[test]
    fn end_session_stops_waits_and_names_why() {
        assert_eq!(
            decide(WM_ENDSESSION, 1, ENDSESSION_LOGOFF as isize),
            EndAction::Stop {
                reason: "Windows sign-out",
                wait: true
            }
        );
        assert_eq!(
            decide(WM_ENDSESSION, 1, 0),
            EndAction::Stop {
                reason: "Windows shutdown or restart",
                wait: true
            }
        );
        assert_eq!(
            decide(
                WM_ENDSESSION,
                1,
                (ENDSESSION_LOGOFF | ENDSESSION_CRITICAL) as isize
            ),
            EndAction::Stop {
                reason: "Windows sign-out",
                wait: true
            },
            "a forced sign-out is still a sign-out"
        );
        assert_eq!(
            decide(WM_ENDSESSION, 1, ENDSESSION_CLOSEAPP as isize),
            EndAction::Stop {
                reason: "an installer closing it (Restart Manager)",
                wait: true
            },
            "Restart Manager, as during an MSI upgrade, wins over the other bits"
        );
        assert_eq!(
            decide(
                WM_ENDSESSION,
                1,
                (ENDSESSION_CLOSEAPP | ENDSESSION_LOGOFF) as isize
            ),
            EndAction::Stop {
                reason: "an installer closing it (Restart Manager)",
                wait: true
            }
        );
    }

    #[test]
    fn the_logoff_bit_is_read_from_a_sign_extended_lparam_too() {
        // On a 64-bit Windows the 0x80000000 bit may arrive sign-extended.
        let extended = ENDSESSION_LOGOFF as i32 as isize;
        assert!(extended < 0);
        assert_eq!(
            decide(WM_ENDSESSION, 1, extended),
            EndAction::Stop {
                reason: "Windows sign-out",
                wait: true
            }
        );
    }

    #[test]
    fn a_cancelled_end_session_is_ignored() {
        assert_eq!(
            decide(WM_ENDSESSION, 0, ENDSESSION_LOGOFF as isize),
            EndAction::Ignore
        );
        assert_eq!(decide(WM_ENDSESSION, 0, 0), EndAction::Ignore);
    }

    #[test]
    fn close_stops_without_waiting() {
        assert_eq!(
            decide(WM_CLOSE, 0, 0),
            EndAction::Stop {
                reason: "a close request (WM_CLOSE)",
                wait: false
            }
        );
    }

    #[test]
    fn the_quit_message_destroys_and_anything_else_is_passed_on() {
        assert_eq!(decide(QUIT_MESSAGE, 0, 0), EndAction::Quit);
        assert_eq!(decide(WM_DESTROY, 0, 0), EndAction::Destroyed);
        for other in [0x0001, 0x0081, 0x0400, 0x8000, 0x8002] {
            assert_eq!(decide(other, 7, 7), EndAction::PassOn, "{other:#x}");
        }
    }

    #[test]
    fn the_stop_is_signalled_once_to_whoever_waits() {
        let stopped = stopped_receiver();
        notify_stopped();
        assert_eq!(
            stopped.recv_timeout(std::time::Duration::from_secs(1)),
            Ok(())
        );
        notify_stopped();
        assert!(
            stopped
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "the signal is sent once"
        );
    }

    #[test]
    fn the_wait_fits_inside_the_five_seconds_windows_gives() {
        assert!(ENDSESSION_WAIT < std::time::Duration::from_secs(5));
        assert!(ENDSESSION_WAIT >= std::time::Duration::from_secs(3));
    }
}
