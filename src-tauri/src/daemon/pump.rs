//! Win32 message loop and tray icon for the daemon. WS3 task 3.3.
//!
//! v1 built the tray on Tauri's event loop, which is fine for a process that
//! has one. The daemon has no Tauri and no window, so it runs the loop itself:
//! `tray-icon` and `muda` for the icon and the menu, and a plain
//! `GetMessage`/`DispatchMessage` pump on the main thread.
//!
//! This is the concrete reason `daemon::run` is not just `#[tokio::main]`. A
//! tray icon is a window-station object: its messages arrive on the thread that
//! created it, and that thread has to be pumping a message queue or nothing
//! ever fires. So the main thread belongs to Win32, and the tokio runtime lives
//! beside it (implementation plan §4.3).
//!
//! ## The menu is three items, and stays three
//!
//! Open, Settings, Quit. v1 considered and rejected a "Start/Stop recording"
//! entry because `start_recording` races the state machine, which does not know
//! about the call; promoting a dev affordance to a shipped one would ship a
//! known bug. Nothing about the split changes that.
//!
//! ## Handlers run on this thread, so they must not block
//!
//! `muda` and `tray-icon` deliver events by calling a handler on whichever
//! thread received the window message, which is this one. Anything slow done
//! there freezes the menu, the icon and every other message. So a handler does
//! exactly one thing: put a `TrayCommand` on a channel. The work happens on the
//! tokio side, where waiting is free.
//!
//! The one exception is `Quit`'s confirmation, which is a `MessageBoxW` and is
//! *supposed* to block this thread: it is a modal dialog, and the tray being
//! unresponsive while a modal is up is what a modal is.
//!
//! ## No tests, for `tray.rs`'s reason and one more
//!
//! Everything here is Win32 and cannot run without a window station. A
//! `cargo test` binary that reached this code would also carry the same GUI
//! import stack `state_machine::supervisor` documents dying on. What can be
//! tested is the decision this file makes, and that is `should_confirm_quit`
//! below, which is pure and has its own tests.

/// What the tray is asking the daemon to do.
///
/// A message rather than a call, because the sender is the Win32 thread and the
/// receiver is the async side. Both halves are cheap, which matters: the sender
/// is holding up the message queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayCommand {
    /// Show the UI, optionally on a particular view.
    ///
    /// `None` is "open the app", `Some("settings")` is the Settings item. The
    /// string is the view name `router.ts` understands, which is the same
    /// vocabulary v1's tray used when it could navigate a window directly.
    ShowUi(Option<String>),
    /// Stop, having finalized whatever is being recorded.
    Quit,
}

/// Whether quitting needs to ask first.
///
/// Pure, and separated from everything around it so the one decision in this
/// file can be tested: the rest is Win32 calls whose behaviour only a Windows
/// desktop can show.
///
/// The question is not "is the app busy" but "would quitting lose something a
/// person cannot get back". A recording in flight is exactly that: the file on
/// disk is a fragment until finalize writes the row, muxes it and attaches the
/// markers.
pub fn should_confirm_quit(recording: bool) -> bool {
    recording
}

#[cfg(windows)]
mod win32 {
    use std::sync::mpsc::Sender;

    use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MB_ICONWARNING, MB_YESNO, IDYES, MSG, MessageBoxW,
        PostThreadMessageW, TranslateMessage, WM_QUIT,
    };
    use windows::core::{HSTRING, PCWSTR};

    use super::TrayCommand;
    use crate::{error, info, warn};

    /// Menu item ids. `muda` hands these back on the event, and matching on a
    /// string is how a menu with three items stays readable.
    const MENU_OPEN: &str = "tray-open";
    const MENU_SETTINGS: &str = "tray-settings";
    const MENU_QUIT: &str = "tray-quit";

    /// The thread running `message_loop`, so `stop` can reach it.
    ///
    /// **Zero means the pump is not running**, which is a real state rather
    /// than an impossible one: `run` can fail before the loop starts, and off
    /// Windows there is no loop at all.
    ///
    /// A thread id rather than a handle or a window, because `WM_QUIT` is a
    /// thread message. It has no window, cannot be sent with `PostMessageW`,
    /// and is what `GetMessageW` returns 0 for.
    static PUMP_THREAD: AtomicU32 = AtomicU32::new(0);

    /// Builds the tray and runs the message loop until Quit.
    ///
    /// **Blocks the calling thread**, which must be the process's main thread:
    /// a tray icon's messages arrive on the thread that created it.
    ///
    /// `commands` carries what the user clicked to whoever is driving the
    /// daemon. Returns when the loop ends, which is only ever after a `Quit`.
    pub fn run(commands: Sender<TrayCommand>) -> Result<(), String> {
        let open = MenuItem::with_id(MENU_OPEN, "Open ninja-recorder", true, None);
        let settings = MenuItem::with_id(MENU_SETTINGS, "Settings", true, None);
        let quit = MenuItem::with_id(MENU_QUIT, "Quit", true, None);
        // Two separators, not one used twice: a menu item belongs to one
        // position in one menu, and appending the same instance twice is asking
        // the same object to be in two places.
        let above = PredefinedMenuItem::separator();
        let below = PredefinedMenuItem::separator();
        let menu = Menu::new();
        menu.append_items(&[&open, &above, &settings, &below, &quit])
            .map_err(|e| format!("cannot build the tray menu: {e}"))?;

        // The handlers run on this thread, inside `DispatchMessageW`. Each one
        // sends and returns; nothing else may happen here.
        let clicks = commands.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id().as_ref() {
                MENU_OPEN => TrayCommand::ShowUi(None),
                MENU_SETTINGS => TrayCommand::ShowUi(Some("settings".to_string())),
                MENU_QUIT => TrayCommand::Quit,
                other => {
                    warn!("tray", "unknown menu id: {other}");
                    return;
                }
            };
            // A closed channel means the daemon is already shutting down, which
            // is not worth a message: the menu is about to go away with it.
            let _ = clicks.send(command);
        }));

        let icon_clicks = commands.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            // Left-click opens the window; the menu is the right-click gesture,
            // which is what every other tray app on Windows does. v1's tray
            // made the same choice and this keeps it.
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = event
            {
                let _ = icon_clicks.send(TrayCommand::ShowUi(None));
            }
        }));

        // **`with_menu_on_left_click(false)` is load-bearing.** `tray-icon`
        // defaults it to *true*, so the menu opened on a left click as well as
        // a right one, and the handler above fired underneath it. The comment
        // there describes the intent correctly and the default quietly
        // contradicted it: left-click is "open the app" on Windows, and a menu
        // that appears for both gestures makes the icon's primary action
        // unreachable.
        let mut builder = TrayIconBuilder::new()
            .with_tooltip("ninja-recorder")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false);
        match icon() {
            Some(icon) => builder = builder.with_icon(icon),
            // Without an icon the tray is an invisible click target. v1 skips
            // the tray rather than ship that; the daemon cannot, because the
            // tray is the only way to reach it at all. So it is built anyway
            // and the problem is said out loud.
            None => error!("tray", "no icon could be loaded; the tray will be invisible"),
        }
        let _tray = builder.build().map_err(|e| format!("cannot create the tray icon: {e}"))?;

        // Recorded before the loop and cleared after it, so `stop` can tell a
        // running pump from one that never started or has already ended.
        // SAFETY: no preconditions; it reads the calling thread's own id.
        PUMP_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

        info!("tray", "tray icon up; pumping messages");
        message_loop();
        PUMP_THREAD.store(0, Ordering::SeqCst);
        Ok(())
    }

    /// The loop itself.
    ///
    /// `GetMessageW` returns 0 on `WM_QUIT`, which is what `stop` below posts
    /// to this thread, and -1 on error. Anything else is a message to dispatch.
    ///
    /// A null window handle asks for messages for *any* window on this thread
    /// and for thread messages, which is what makes a posted `WM_QUIT` with no
    /// window of its own arrive here at all.
    fn message_loop() {
        let mut message = MSG::default();
        loop {
            // SAFETY: `message` is a valid writable `MSG`. A null window handle
            // asks for messages for any window on this thread, which is what a
            // thread whose windows are owned by `tray-icon` wants.
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                // 0 is `WM_QUIT` and the ordinary way out. -1 is an error, and
                // continuing would spin on it forever, so both end the loop.
                if result.0 < 0 {
                    error!("tray", "the message loop failed; the tray is gone");
                }
                return;
            }
            // SAFETY: `message` was filled in by the `GetMessageW` above.
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Ends the message loop, from any thread.
    ///
    /// **This used to be `PostQuitMessage`, and that was a bug.** That function
    /// posts `WM_QUIT` to the *calling* thread's queue, and its only caller is
    /// the tray command handler, which `daemon::pump_until_quit` runs on a
    /// thread of its own precisely because the pump thread is blocked inside
    /// `GetMessageW`. So the quit was posted to a thread with no message loop,
    /// where it sat forever, and the pump went on pumping. Tray Quit showed its
    /// confirmation, took "yes" for an answer, and did nothing: the daemon kept
    /// running and kept recording, and the only way to stop it was Task
    /// Manager.
    ///
    /// It survived because the two halves are tested separately and the seam
    /// between them is a thread boundary. `should_confirm_quit` is a pure
    /// function with tests; the message loop is Windows-only and has none; and
    /// the failure needs a real tray, a real click, and someone to answer
    /// "yes", which is the one combination nothing in CI or the dev loop
    /// reaches.
    ///
    /// `PostThreadMessageW` addresses the pump thread by id instead. `WM_QUIT`
    /// is a thread message with no window, so this is the posting function for
    /// it; `PostMessageW` refuses it outright.
    pub fn stop() {
        let pump = PUMP_THREAD.load(Ordering::SeqCst);
        if pump == 0 {
            // Nothing to stop. Worth a line rather than silence: it means the
            // tray never came up, and the caller believes it just quit.
            warn!("tray", "asked to stop, but no message loop is running");
            return;
        }
        // SAFETY: `pump` is a thread id this process recorded from
        // `GetCurrentThreadId`, and `WM_QUIT` carries no pointers in either
        // parameter. A thread that has since exited makes this fail, which is
        // handled below rather than being undefined.
        if let Err(e) = unsafe { PostThreadMessageW(pump, WM_QUIT, WPARAM(0), LPARAM(0)) } {
            error!("tray", "could not ask the message loop to stop: {e}");
        }
    }

    /// Asks before quitting mid-recording.
    ///
    /// A `MessageBoxW` rather than a notification, which is what the plan
    /// sketched: a notification is a statement, and this is a question whose
    /// answer decides whether a game is lost. Modal on purpose, and blocking
    /// this thread is what modal means.
    ///
    /// Returns whether the user said yes.
    pub fn confirm_quit_while_recording() -> bool {
        let text = HSTRING::from(
            "ninja-recorder is recording a game right now.\n\n\
             Quitting will finish and save that recording first, which takes a \
             few seconds. Quit anyway?",
        );
        let caption = HSTRING::from("Quit ninja-recorder?");
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call,
        // and a null owner window is valid for a process with no windows of its
        // own.
        let answer = unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), MB_YESNO | MB_ICONWARNING)
        };
        answer == IDYES
    }

    /// The tray icon, from wherever this build put it.
    ///
    /// Four places, in order of how much they can be trusted:
    ///
    /// 1. The executable's own resources, which is where `tauri-build` embeds
    ///    `icon.ico`. Ordinal 1 is what its generated resource script uses.
    /// 2. By resource *name*, in case that ordinal ever changes.
    /// 3. `icons/icon.ico` beside the executable.
    /// 4. The copy compiled into this binary, written out to disk.
    ///
    /// Tried in that order rather than picking one, because which of them works
    /// is a property of how the binary was built: a daemon started from an
    /// installer, from `cargo run`, or from a CI job should all get an icon.
    ///
    /// **The fourth is there because CI proved the first three are not enough.**
    /// The opening run of `scripts/smoke-daemon.ps1` on a `cargo build` binary
    /// logged all three failing and "the tray will be invisible": a plain cargo
    /// build embeds no resource, and `target/debug` has no `icons/` beside it.
    /// An invisible tray is the one failure this process cannot survive, since
    /// the tray is the only way to reach the app.
    fn icon() -> Option<Icon> {
        if let Ok(icon) = Icon::from_resource(1, None) {
            return Some(icon);
        }
        if let Ok(icon) = Icon::from_resource_name("icon", None) {
            return Some(icon);
        }

        if let Some(beside_exe) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("icons").join("icon.ico")))
            && let Ok(icon) = Icon::from_path(&beside_exe, None)
        {
            return Some(icon);
        }

        embedded_icon()
    }

    /// The icon this binary carries, written somewhere `LoadImageW` can read it.
    ///
    /// `tray-icon` loads from a path or from a resource, and a given build may
    /// have neither. It does have the bytes: `include_bytes!` puts
    /// `icons/icon.ico` in the binary, 85 KB against a 600 KB executable, and
    /// buys a daemon that always has an icon.
    ///
    /// Written on every start rather than only when absent, because a truncated
    /// file left by an earlier run would fail in exactly the way this exists to
    /// prevent, and 85 KB to the temp directory once per start is nothing next
    /// to being unreachable.
    fn embedded_icon() -> Option<Icon> {
        const ICO: &[u8] = include_bytes!("../../icons/icon.ico");

        let path = std::env::temp_dir().join("ninja-recorder-tray.ico");
        if let Err(e) = std::fs::write(&path, ICO) {
            warn!("tray", "could not write the embedded icon to {}: {e}", path.display());
            return None;
        }
        match Icon::from_path(&path, None) {
            Ok(icon) => Some(icon),
            Err(e) => {
                warn!("tray", "the embedded icon would not load from {}: {e}", path.display());
                None
            }
        }
    }
}

/// The same three functions on a platform with no tray.
///
/// **Not a stub that lies.** `run` reports that there is no tray, which is
/// true, and the caller's answer to that is the same as its answer to a tray
/// that failed to build: wait for Ctrl-C instead. That is what makes
/// `daemon::pump_until_quit` one code path compiled everywhere rather than two
/// behind a `cfg`, and one of them is the path a dev box can compile.
///
/// The daemon is Windows software. The point of this module is not to support
/// another platform, it is to keep the *shape* of the Windows code in front of
/// a compiler that runs in seconds.
#[cfg(not(windows))]
mod elsewhere {
    use std::sync::mpsc::Sender;

    use super::TrayCommand;

    pub fn run(_commands: Sender<TrayCommand>) -> Result<(), String> {
        Err("this platform has no system tray; the daemon runs headless".to_string())
    }

    /// Unreachable: `run` above returns before any command can be sent, so
    /// nothing ever asks the loop that does not exist to stop.
    pub fn stop() {}

    /// Likewise unreachable. `true` rather than `false` so that if it ever did
    /// run, it would let the quit through and finalize rather than refuse to
    /// stop.
    pub fn confirm_quit_while_recording() -> bool {
        true
    }
}

#[cfg(windows)]
pub use win32::{confirm_quit_while_recording, run, stop};
#[cfg(not(windows))]
pub use elsewhere::{confirm_quit_while_recording, run, stop};

#[cfg(test)]
mod tests {
    use super::*;

    /// The only decision in this file that is not a Win32 call.
    ///
    /// Quitting with nothing in flight must not put a dialog in front of
    /// someone: the tray's Quit is a deliberate act, and asking twice teaches
    /// people to dismiss the question they should read.
    #[test]
    fn quitting_while_idle_asks_nothing() {
        assert!(!should_confirm_quit(false));
    }

    /// And quitting mid-game must, because the thing at stake is a recording
    /// that cannot be re-made.
    #[test]
    fn quitting_mid_recording_asks_first() {
        assert!(should_confirm_quit(true));
    }
}
