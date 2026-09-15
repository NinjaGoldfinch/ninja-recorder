//! How the process was asked to start.
//!
//! One binary serves two roles ([DEVELOPMENT.md §12](../../DEVELOPMENT.md)):
//! the UI, and — once it exists — a headless recorder daemon. Which one is
//! decided by argv, and that decision has to be settled *before* either the
//! tray or the daemon is built, because it becomes an on-disk contract the
//! moment autostart writes it into the registry. `tauri-plugin-autostart`
//! records the flag once, at enable time, so a flag that changes meaning later
//! silently strands every user who turned autostart on before the change.
//!
//! Parsing lives here, away from `lib.rs`, so it can be unit tested without a
//! Tauri runtime — and so it names no `tauri` type, for the reason
//! `core`'s header gives.
//!
//! The contract is no longer hypothetical: `lib.rs` registers
//! `tauri-plugin-autostart` with `HIDDEN_FLAG`, so on any machine where the
//! user has turned start-on-login on, that exact string is sitting in the
//! registry waiting to be handed back to a future build. The constants below
//! are what both sides read, so the flag can't be changed on one side only.

/// Start with no window, sitting in the tray.
///
/// This is the string autostart writes into `HKCU\...\Run`, which is why it
/// is a constant rather than a literal in two places: the parser and the
/// registration have to agree forever, including with builds that wrote the
/// entry years earlier.
pub const HIDDEN_FLAG: &str = "--hidden";

/// Run headless. What `daemon::run` answers to, and what `daemon::spawn` hands
/// the executable when the UI finds nobody listening.
///
/// Not yet what autostart registers. See `autostart_args`.
pub const DAEMON_FLAG: &str = "--daemon";

/// The arguments written into `HKCU\...\Run` when the user ticks start-on-login.
///
/// ## Why this is still `--hidden` when the daemon works
///
/// WS3.5's exit criterion is that login starts a daemon and nothing else, and
/// the daemon has run since WS3.2 — so the obvious change is to put
/// `DAEMON_FLAG` here. It is deliberately not made yet, because today it would
/// leave an autostart user worse off than before:
///
/// - The daemon has no tray icon and no notifications until WS3.3 builds its
///   Win32 message pump. A login start would be a recorder with no window, no
///   icon and no way to reach it short of the Start menu.
/// - The UI still builds a supervisor of its own until the rest of WS3.4 moves
///   it onto the pipe. Opening the app after a daemon had started at login
///   would mean two state machines watching for the same game and two
///   recorders reaching for the same capture device.
///
/// So the flag moves when those two are true, and this function plus the test
/// below is what makes that a one-line change in a place that is checked rather
/// than a string to go and find in `lib.rs`'s builder.
///
/// ## Why it is a function and not a `const`
///
/// The registry holds whatever was written the day the box was ticked, and
/// hands it back to whatever build is installed years later. That makes the
/// value an on-disk contract, and a contract is worth stating in one place that
/// a test can reach. `lib.rs` registers what this returns; nothing else decides
/// it.
pub fn autostart_args() -> Vec<&'static str> {
    vec![HIDDEN_FLAG]
}

/// What this process should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Launch {
    /// Normal start: create the main window and show it.
    #[default]
    Ui,
    /// Start with no window at all.
    ///
    /// Note this creates *no* window rather than a hidden one. A window
    /// configured `visible: false` still constructs the WebView2 instance and
    /// costs the full webview footprint, which defeats the point — the
    /// intended caller is autostart-on-login, which wants to sit in the tray
    /// costing nothing until asked for.
    UiHidden,
    /// Headless recorder daemon.
    ///
    /// `main.rs` dispatches this straight to `daemon::run`, which since WS3.2
    /// opens the library, brings up the supervisor and serves clients over the
    /// pipe. The refusal that used to live here as a string became a value
    /// returned by the daemon, and then a function body: filling WS3 in meant
    /// implementing `run`, not rerouting a process.
    Daemon,
}

impl Launch {
    /// Reads the mode out of an argument list (excluding argv[0]).
    ///
    /// Deliberately hand-rolled rather than a CLI crate: there are two flags,
    /// no values, and no help text worth generating. Unknown arguments are
    /// ignored rather than rejected — Windows and macOS both hand a launched
    /// app arguments we never asked for, and refusing to start over one would
    /// be a bad trade.
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut mode = Launch::Ui;
        for arg in args {
            match arg.as_ref() {
                // `--daemon` wins: it is the stronger statement, and a
                // daemon has no window to hide.
                DAEMON_FLAG => return Launch::Daemon,
                HIDDEN_FLAG => mode = Launch::UiHidden,
                _ => {}
            }
        }
        mode
    }

    /// Reads the mode from the real process arguments.
    pub fn from_env() -> Self {
        Self::from_args(std::env::args().skip(1))
    }

    /// Whether a main window should be created at startup.
    pub fn creates_window(self) -> bool {
        matches!(self, Launch::Ui)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_is_a_normal_ui_start() {
        assert_eq!(Launch::from_args(Vec::<String>::new()), Launch::Ui);
        assert!(Launch::from_args::<_, &str>([]).creates_window());
    }

    #[test]
    fn hidden_starts_without_a_window() {
        let mode = Launch::from_args(["--hidden"]);
        assert_eq!(mode, Launch::UiHidden);
        assert!(
            !mode.creates_window(),
            "a hidden start must create no window, not a hidden one"
        );
    }

    /// Parsing `--daemon` and *implementing* it are separate questions, and
    /// this file only answers the first. A build that quietly read the flag
    /// as a normal start would look like it worked while recording nothing
    /// in the background, so the mode has to survive parsing distinctly;
    /// whether it can then run is `daemon::run`'s answer.
    #[test]
    fn daemon_is_recognised_as_its_own_mode() {
        let mode = Launch::from_args(["--daemon"]);
        assert_eq!(mode, Launch::Daemon);
        assert_ne!(mode, Launch::Ui, "the daemon must not fall back to a window");
        assert!(!mode.creates_window());
    }

    #[test]
    fn daemon_wins_over_hidden_regardless_of_order() {
        assert_eq!(Launch::from_args(["--hidden", "--daemon"]), Launch::Daemon);
        assert_eq!(Launch::from_args(["--daemon", "--hidden"]), Launch::Daemon);
    }

    #[test]
    fn unknown_arguments_are_ignored_rather_than_fatal() {
        // Both OSes hand launched apps arguments we never asked for — macOS
        // adds `-psn_...` on some launch paths, Windows passes shell verbs.
        assert_eq!(
            Launch::from_args(["-psn_0_12345", "--hidden", "/unexpected"]),
            Launch::UiHidden
        );
        assert_eq!(Launch::from_args(["--not-a-flag"]), Launch::Ui);
    }

    #[test]
    fn only_the_ui_mode_creates_a_window() {
        assert!(Launch::Ui.creates_window());
        assert!(!Launch::UiHidden.creates_window());
        assert!(!Launch::Daemon.creates_window());
    }

    /// The registry holds the *string*, so renaming the constant is free and
    /// changing its value is not: an installed `HKCU\...\Run` entry written
    /// by an older build would stop meaning anything, and start-on-login would
    /// silently open a window instead of going to the tray.
    /// The deferral, pinned. This is not a test that `--hidden` is *right* —
    /// it is a test that changing it is a decision someone made on purpose,
    /// having read why it was left alone. When WS3.3 and WS3.4 land, this test
    /// and `autostart_args` change together.
    #[test]
    fn autostart_still_registers_the_ui_rather_than_the_daemon() {
        assert_eq!(
            autostart_args(),
            vec![HIDDEN_FLAG],
            "flipping this to DAEMON_FLAG needs the daemon's tray (WS3.3) and a UI that \
             stops running its own supervisor (WS3.4) — see autostart_args"
        );
        // And whatever is registered has to parse back to a mode that does not
        // open a window, or start-on-login becomes start-a-window-on-login.
        assert!(!Launch::from_args(autostart_args()).creates_window());
    }

    #[test]
    fn the_autostart_flag_is_the_exact_string_already_in_the_registry() {
        assert_eq!(HIDDEN_FLAG, "--hidden");
        assert_eq!(DAEMON_FLAG, "--daemon");
        assert_eq!(Launch::from_args([HIDDEN_FLAG]), Launch::UiHidden);
    }
}
