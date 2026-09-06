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

/// Run headless. Reserved — see `Launch::Daemon`.
pub const DAEMON_FLAG: &str = "--daemon";

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
    /// Headless recorder daemon. Reserved: recognised so the flag's meaning is
    /// fixed now, but not yet implemented — see `Launch::unsupported`.
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

    /// The reason this mode can't run yet, if it can't.
    ///
    /// `Daemon` is parsed but unimplemented. Returning a message rather than
    /// silently falling back to a normal window matters: autostart registers
    /// `HIDDEN_FLAG` today and is the obvious caller to move onto `--daemon`
    /// once it exists, and a build that quietly ignored the flag would look
    /// like it worked while recording nothing in the background.
    pub fn unsupported(self) -> Option<&'static str> {
        match self {
            Launch::Daemon => Some(
                "--daemon is reserved for the headless recorder daemon, which is not built yet",
            ),
            _ => None,
        }
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

    #[test]
    fn daemon_is_recognised_but_reports_itself_unsupported() {
        let mode = Launch::from_args(["--daemon"]);
        assert_eq!(mode, Launch::Daemon);
        assert!(
            mode.unsupported().is_some(),
            "an unimplemented mode must say so rather than fall back to a window"
        );
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
    #[test]
    fn the_autostart_flag_is_the_exact_string_already_in_the_registry() {
        assert_eq!(HIDDEN_FLAG, "--hidden");
        assert_eq!(DAEMON_FLAG, "--daemon");
        assert_eq!(Launch::from_args([HIDDEN_FLAG]), Launch::UiHidden);
    }

    #[test]
    fn supported_modes_report_no_error() {
        assert!(Launch::Ui.unsupported().is_none());
        assert!(Launch::UiHidden.unsupported().is_none());
    }
}
