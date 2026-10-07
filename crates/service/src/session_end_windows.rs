//! The hidden top-level window that tells a Windows daemon its session is
//! ending, so it stops cleanly instead of being ended without a word. It
//! lives on its own std thread with a `GetMessageW` loop, never touches the
//! tokio runtime, and reaches the daemon only through a trigger
//! (`Shared::trigger_shutdown`) and a channel the daemon fills when its stop
//! is done (`session_end::notify_stopped`). It is never shown, so nothing
//! flashes and no taskbar button appears.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, Once};
use std::time::Duration;

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SetWindowLongPtrW, TranslateMessage, WM_NCCREATE, WNDCLASSEXW, WS_OVERLAPPED,
};

#[cfg(test)]
use crate::session_end::{
    ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF, ENDSESSION_WAIT, WM_CLOSE,
    WM_DESTROY, WM_ENDSESSION, WM_QUERYENDSESSION,
};
use crate::session_end::{EndAction, QUIT_MESSAGE, decide};

/// The window class every daemon's window uses. Fixed, so a test (and a
/// person with Spy++) finds it; the title tells two daemons apart.
pub(crate) const WINDOW_CLASS: &str = "CrystallineSessionEnd";

/// The title of a daemon's window: its pid, so `FindWindowW` finds one daemon.
pub(crate) fn window_title(pid: u32) -> String {
    format!("Crystalline daemon {pid}")
}

pub(crate) type Trigger = Box<dyn Fn(&'static str) + Send + Sync>;

/// What the window procedure reaches through `GWLP_USERDATA`.
struct Hook {
    trigger: Trigger,
    stopped: Mutex<Receiver<()>>,
    wait: Duration,
    done: AtomicBool,
}

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The running window. Dropping it destroys the window and joins its thread.
pub(crate) struct SessionEndWindow {
    hwnd: usize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SessionEndWindow {
    /// Start the window on its own thread. `None`, with a warning, when the
    /// window cannot be made: the daemon works on, only without a clean stop
    /// at sign-out.
    pub(crate) fn start(
        title: &str,
        trigger: Trigger,
        stopped: Receiver<()>,
        wait: Duration,
    ) -> Option<SessionEndWindow> {
        let title = wide(title);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<usize, u32>>(1);
        let hook = Box::new(Hook {
            trigger,
            stopped: Mutex::new(stopped),
            wait,
            done: AtomicBool::new(false),
        });
        let thread = std::thread::Builder::new()
            .name("session-end-window".into())
            .spawn(move || run(title, hook, ready_tx))
            .map_err(|e| {
                tracing::warn!("no clean stop at sign-out: the window thread did not start ({e})")
            })
            .ok()?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(hwnd)) => Some(SessionEndWindow {
                hwnd,
                thread: Some(thread),
            }),
            Ok(Err(code)) => {
                tracing::warn!(
                    "no clean stop at sign-out: the session-end window could not be created (error {code})"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    "no clean stop at sign-out: the session-end window did not start within 5s"
                );
                None
            }
        }
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd as HWND
    }
}

impl Drop for SessionEndWindow {
    fn drop(&mut self) {
        // SAFETY: posting to the window from whatever thread drops this
        // handle; PostMessageW may cross threads and only queues the message
        // for the window's own thread. A window that is already gone makes
        // the call fail, which is fine.
        unsafe { PostMessageW(self.hwnd(), QUIT_MESSAGE, 0, 0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

static REGISTER: Once = Once::new();

/// The window thread: register the class once per process, create the
/// window with the hook as its creation parameter, pump messages until
/// `WM_QUIT`, then free the hook.
fn run(title: Vec<u16>, hook: Box<Hook>, ready: std::sync::mpsc::SyncSender<Result<usize, u32>>) {
    let class = wide(WINDOW_CLASS);
    // SAFETY: a null module name asks for this executable's own handle.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    REGISTER.call_once(|| {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        // SAFETY: a fully initialized class description; the class name is
        // copied by the call.
        let atom = unsafe { RegisterClassExW(&wc) };
        if atom == 0 {
            // SAFETY: reads this thread's last error, set by the failed call.
            let code = unsafe { GetLastError() };
            // Once per process, inside call_once. CreateWindowExW fails next
            // and says so too.
            tracing::warn!("the session-end window class could not be registered (error {code})");
        }
    });
    let hook = Box::into_raw(hook);
    // SAFETY: a registered class, no parent (a top-level window, never
    // HWND_MESSAGE, which receives no broadcasts), WS_OVERLAPPED without
    // WS_VISIBLE (never shown), zero size; the hook pointer is read back in
    // WM_NCCREATE and outlives the window (freed after the loop below).
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            hook as *const core::ffi::c_void,
        )
    };
    if hwnd.is_null() {
        // SAFETY: reads this thread's last error, set by the failed call.
        let code = unsafe { GetLastError() };
        let _ = ready.send(Err(code));
        // SAFETY: CreateWindowExW failed, so no window holds the pointer.
        drop(unsafe { Box::from_raw(hook) });
        return;
    }
    let _ = ready.send(Ok(hwnd as usize));
    let mut msg = MSG::default();
    // The standard loop over this thread's queue; 0 is WM_QUIT and -1 an
    // error, both end it.
    let ended = loop {
        // SAFETY: a valid out parameter; a null window reads every message
        // of this thread.
        let got = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
        if got <= 0 {
            break got;
        }
        // SAFETY: a message GetMessageW just filled.
        unsafe { TranslateMessage(&msg) };
        // SAFETY: as above.
        unsafe { DispatchMessageW(&msg) };
    };
    if ended < 0 {
        // SAFETY: reads this thread's last error, set by GetMessageW.
        let code = unsafe { GetLastError() };
        tracing::warn!("the session-end window stopped reading messages (error {code})");
        // SAFETY: destroying our own window on its own thread, so it is gone
        // before the hook below is freed.
        unsafe { DestroyWindow(hwnd) };
    }
    // SAFETY: the window is destroyed (WM_QUIT follows WM_DESTROY, and after
    // a GetMessageW error the call above destroyed it), so no procedure call
    // can read the hook any more.
    drop(unsafe { Box::from_raw(hook) });
}

/// The window procedure: [`decide`] says what to do.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE, lParam points at the CREATESTRUCTW whose
        // lpCreateParams is the hook `run` passed.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        // SAFETY: storing a pointer in this window's own user data.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    // SAFETY: reading this window's own user data, zero before WM_NCCREATE.
    let data = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    // SAFETY: zero (no hook yet) or the hook `run` keeps alive until the
    // window is gone.
    let hook = unsafe { (data as *const Hook).as_ref() };
    match (decide(message, wparam, lparam), hook) {
        (EndAction::AllowEnd, _) => 1,
        (EndAction::Ignore, _) => 0,
        (EndAction::Stop { reason, wait }, Some(hook)) => {
            tracing::info!("Windows asks the daemon to stop: {reason}");
            (hook.trigger)(reason);
            // Hold WM_ENDSESSION until the daemon's stop is done: returning
            // is Windows' permission to end the process. Bounded, so the
            // procedure always returns inside Windows' 5 s.
            if wait && !hook.done.swap(true, Ordering::SeqCst) {
                let stopped = hook
                    .stopped
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if stopped.recv_timeout(hook.wait).is_err() {
                    tracing::warn!(
                        "the daemon did not finish stopping within {}s of the session end; the index is crash safe",
                        hook.wait.as_secs()
                    );
                }
            }
            0
        }
        (EndAction::Quit, _) => {
            // SAFETY: destroying our own window on its own thread.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        (EndAction::Destroyed, _) => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::SyncSender;
    use std::time::Instant;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GA_PARENT, GetAncestor, GetDesktopWindow, IsWindow, IsWindowVisible,
        SendMessageW,
    };

    /// A started window, the reasons its trigger recorded, and the sender a
    /// test uses to say "the stop is done".
    type Started = (
        SessionEndWindow,
        Arc<Mutex<Vec<&'static str>>>,
        SyncSender<()>,
    );

    /// A window with a title of its own, a trigger that records each reason,
    /// and the sender a test uses to say "the stop is done".
    fn window(tag: &str, wait: Duration) -> Started {
        let fired = Arc::new(Mutex::new(Vec::new()));
        let log = fired.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let title = format!("Crystalline test {tag} {}", std::process::id());
        let window = SessionEndWindow::start(
            &title,
            Box::new(move |reason| log.lock().unwrap().push(reason)),
            done_rx,
            wait,
        )
        .expect("the window starts");
        (window, fired, done_tx)
    }

    #[test]
    fn the_window_is_a_hidden_top_level_window_found_by_its_class() {
        let (window, _, _) = window("found", ENDSESSION_WAIT);
        let class = wide(WINDOW_CLASS);
        let title = wide(&format!("Crystalline test found {}", std::process::id()));
        // SAFETY: two NUL-terminated wide strings that live for the call.
        let found = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
        assert_eq!(
            found,
            window.hwnd(),
            "FindWindowW finds it by class and title"
        );
        // SAFETY: a valid window handle.
        let parent = unsafe { GetAncestor(window.hwnd(), GA_PARENT) };
        // SAFETY: takes no arguments.
        let desktop = unsafe { GetDesktopWindow() };
        // SAFETY: a valid window handle.
        let visible = unsafe { IsWindowVisible(window.hwnd()) };
        assert_eq!(
            parent, desktop,
            "top-level, not a message-only window (whose parent is HWND_MESSAGE)"
        );
        assert_eq!(visible, 0, "never shown");
    }

    #[test]
    fn query_end_session_answers_true_at_once_and_stops_nothing() {
        let (window, fired, _) = window("query", ENDSESSION_WAIT);
        let started = Instant::now();
        // SAFETY: a valid window handle; SendMessageW waits for the window's
        // own thread to answer.
        let answer = unsafe {
            SendMessageW(
                window.hwnd(),
                WM_QUERYENDSESSION,
                0,
                ENDSESSION_LOGOFF as isize,
            )
        };
        assert_eq!(answer, 1);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(fired.lock().unwrap().is_empty());
    }

    #[test]
    fn end_session_stops_the_daemon_and_holds_until_the_stop_is_done() {
        let (window, fired, done) = window("end", ENDSESSION_WAIT);
        let watcher = {
            let fired = fired.clone();
            std::thread::spawn(move || {
                while fired.lock().unwrap().is_empty() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::thread::sleep(Duration::from_millis(300));
                done.send(()).unwrap();
            })
        };
        let started = Instant::now();
        // SAFETY: as above.
        let answer =
            unsafe { SendMessageW(window.hwnd(), WM_ENDSESSION, 1, ENDSESSION_LOGOFF as isize) };
        let took = started.elapsed();
        watcher.join().unwrap();
        assert_eq!(answer, 0);
        assert_eq!(*fired.lock().unwrap(), ["Windows sign-out"]);
        assert!(
            took >= Duration::from_millis(300),
            "held until the stop was done: {took:?}"
        );
        assert!(took < ENDSESSION_WAIT, "and no longer: {took:?}");
    }

    #[test]
    fn end_session_returns_at_the_deadline_when_the_stop_never_finishes() {
        let wait = Duration::from_millis(400);
        let (window, fired, _done) = window("deadline", wait);
        let started = Instant::now();
        // SAFETY: as above.
        unsafe { SendMessageW(window.hwnd(), WM_ENDSESSION, 1, 0) };
        let took = started.elapsed();
        assert_eq!(*fired.lock().unwrap(), ["Windows shutdown or restart"]);
        assert!(
            took >= wait && took < wait + Duration::from_secs(2),
            "{took:?}"
        );
    }

    #[test]
    fn a_cancelled_end_session_and_a_close_behave_as_decided() {
        let (window, fired, _) = window("cancel", ENDSESSION_WAIT);
        // SAFETY: as above.
        unsafe { SendMessageW(window.hwnd(), WM_ENDSESSION, 0, ENDSESSION_LOGOFF as isize) };
        assert!(
            fired.lock().unwrap().is_empty(),
            "a cancelled end stops nothing"
        );
        let started = Instant::now();
        // SAFETY: as above.
        let answer = unsafe { SendMessageW(window.hwnd(), WM_CLOSE, 0, 0) };
        assert_eq!(answer, 0);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a close does not wait"
        );
        assert_eq!(*fired.lock().unwrap(), ["a close request (WM_CLOSE)"]);
        // SAFETY: as above.
        let alive = unsafe { IsWindow(window.hwnd()) };
        assert_ne!(alive, 0, "WM_CLOSE does not destroy the window");
    }

    #[test]
    fn dropping_the_window_destroys_it_and_ends_its_thread() {
        let (window, _, _) = window("drop", ENDSESSION_WAIT);
        let hwnd = window.hwnd();
        let started = Instant::now();
        drop(window);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the thread was joined promptly"
        );
        // SAFETY: IsWindow takes any value and answers whether it is a window.
        let alive = unsafe { IsWindow(hwnd) };
        assert_eq!(alive, 0);
    }

    #[test]
    fn two_windows_in_one_process_both_start() {
        let (a, _, _) = window("two-a", ENDSESSION_WAIT);
        let (b, _, _) = window("two-b", ENDSESSION_WAIT);
        assert_ne!(
            a.hwnd(),
            b.hwnd(),
            "the class is registered once and reused"
        );
    }

    #[test]
    fn the_message_ids_match_windows() {
        use windows_sys::Win32::UI::WindowsAndMessaging as w;
        assert_eq!(WM_DESTROY, w::WM_DESTROY);
        assert_eq!(WM_CLOSE, w::WM_CLOSE);
        assert_eq!(WM_QUERYENDSESSION, w::WM_QUERYENDSESSION);
        assert_eq!(WM_ENDSESSION, w::WM_ENDSESSION);
        assert_eq!(QUIT_MESSAGE, w::WM_APP + 1);
        assert_eq!(ENDSESSION_CLOSEAPP, w::ENDSESSION_CLOSEAPP);
        assert_eq!(ENDSESSION_CRITICAL, w::ENDSESSION_CRITICAL);
        assert_eq!(ENDSESSION_LOGOFF, w::ENDSESSION_LOGOFF);
    }
}
