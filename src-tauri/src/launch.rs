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
//! The contract is not hypothetical, and it is the reason there is exactly one
//! flag here rather than two. `--hidden` was what autostart wrote before
//! WS3.5, so on any machine where start-on-login was turned on before that,
//! the string is still sitting in the registry waiting to be handed back. It
//! is **no longer recognised** (#71), which makes it an unknown argument and
//! therefore ignored, so such a login opens a window once. `daemon::autostart`
//! rewrites the entry the first time the daemon runs, so it happens once and
//! not every login. The constant below is what both sides read, so the flag
//! that replaced it can't be changed on one side only.

/// Run headless. What `daemon::run` answers to, and what `daemon::spawn` hands
/// the executable when the UI finds nobody listening.
///
/// What autostart registers. See `autostart_args`.
pub const DAEMON_FLAG: &str = "--daemon";

/// Run as the own backend's capture worker (#241). Only ever passed by the
/// daemon, to its own executable, when League's client opens; never written
/// anywhere a later build could be handed it back, so unlike `--daemon` it is
/// not an on-disk contract. See `recorder::own::worker`.
pub const CAPTURE_WORKER_FLAG: &str = "--capture-worker";

/// The arguments written into `HKCU\...\Run` when the user ticks start-on-login.
///
/// ## `--daemon`, since WS3.5
///
/// Login starts the recorder and nothing else. No window, no WebView2, and
/// nothing that costs anything until the user asks for it: the daemon is the
/// process that must outlive the UI, so it is the one worth starting.
///
/// It waited for two things, and both are now true. The daemon has a tray
/// (WS3.3), so a login start is reachable — which `--hidden` provided and its
/// replacement had to as well. And the UI stopped building a supervisor of its
/// own (WS3.4), so opening the app after a login start no longer means two
/// state machines watching one game.
///
/// ## `--hidden` is gone, and the entries holding it heal themselves
///
/// The registry holds whatever was written the day the box was ticked, and
/// Windows hands it back to whatever build is installed years later. Every user
/// who enabled autostart before WS3.5 still has `--hidden` in their `Run` key.
///
/// It is no longer a flag (#71), so it parses as an unknown argument and is
/// ignored, and such a login is an ordinary start: a window opens. That is the
/// cost of removing it, and it is paid **once**, because `daemon::autostart`
/// rewrites an enabled entry with these arguments the first time the daemon
/// runs. The window that login opened starts a daemon itself, so the rewrite
/// happens on the same login that showed the window, and the next one is a
/// daemon start.
///
/// Keeping it as an alias was the alternative and was rejected: an alias is a
/// second name for `--daemon` that no code path would ever produce, and it
/// would have to be carried forever rather than for a release, because the
/// registry never stops handing back what it was given.
///
/// ## Why it is a function and not a `const`
///
/// The value is an on-disk contract, and a contract is worth stating in one
/// place that a test can reach. `lib.rs` registers what this returns; nothing
/// else decides it.
pub fn autostart_args() -> Vec<&'static str> {
    vec![DAEMON_FLAG]
}

/// What this process should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Launch {
    /// Normal start: create the main window and show it.
    #[default]
    Ui,
    /// Headless recorder daemon.
    ///
    /// `main.rs` dispatches this straight to `daemon::run`, which since WS3.2
    /// opens the library, brings up the supervisor and serves clients over the
    /// pipe. The refusal that used to live here as a string became a value
    /// returned by the daemon, and then a function body: filling WS3 in meant
    /// implementing `run`, not rerouting a process.
    Daemon,
    /// The own backend's capture worker: the session thread, served over
    /// stdin and stdout to the daemon that spawned it.
    ///
    /// Dispatched by `main.rs` before anything else is built. It takes no
    /// single-instance lock, builds no tray, opens no database and binds no
    /// pipe, so it can run beside the daemon that owns all of those.
    CaptureWorker,
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
                // Wins over anything else: a worker must never become a
                // second daemon or a window.
                CAPTURE_WORKER_FLAG => return Launch::CaptureWorker,
                DAEMON_FLAG => mode = Launch::Daemon,
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

    /// The bundle has to name which binary is the application, because this
    /// crate builds two: `ninja-recorder` from `main.rs`, and `gen-contract`,
    /// the contract emitter in `src/bin/`. Tauri bundles every binary Cargo
    /// produces, and the installer's Start Menu shortcut points at whichever
    /// one it considers the main one.
    ///
    /// **This is not hypothetical.** With `mainBinaryName` absent, an
    /// installed build's shortcut pointed at `gen-contract.exe`: a
    /// console-subsystem binary with no `windows_subsystem` attribute, which
    /// opens a console, writes its TypeScript to a repo path that does not
    /// exist on a user's machine, and exits. From outside it is a console
    /// window that appears and closes, with no app and nothing logged anywhere
    /// — the application it was supposed to start never ran. The devtools
    /// config always set the field, which is why only release installs were
    /// affected, and why it looked like a crash rather than a shortcut.
    ///
    /// Pinned against `CARGO_PKG_NAME` rather than a literal: the default bin
    /// target takes the package's name, so these are the same string by
    /// construction and renaming the package cannot leave the config behind.
    #[test]
    fn the_bundle_names_the_application_as_its_main_binary() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["mainBinaryName"].as_str(),
            Some(env!("CARGO_PKG_NAME")),
            "the installer's shortcut points at whatever this names"
        );
    }

    /// And the devtools bundle names its own, which is what `productName`
    /// alone could not do: both configs install side by side, so a shortcut
    /// naming the wrong one would launch the other build's binary.
    #[test]
    fn the_devtools_bundle_names_its_own_main_binary() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.devtools.conf.json")).unwrap();
        assert_eq!(conf["mainBinaryName"].as_str(), conf["productName"].as_str());
    }

    #[test]
    fn no_arguments_is_a_normal_ui_start() {
        assert_eq!(Launch::from_args(Vec::<String>::new()), Launch::Ui);
        assert!(Launch::from_args::<_, &str>([]).creates_window());
    }

    /// **`--hidden` is not a flag any more** (#71), so it falls through the
    /// same branch as `-psn_0_12345`: unknown arguments are ignored, and what
    /// is left is an ordinary start with a window.
    ///
    /// Pinned rather than left implicit, because it is the whole consequence
    /// of the removal. Every `Run` key written before WS3.5 still holds the
    /// string, so this assertion describes what those machines do at their
    /// next login, once.
    #[test]
    fn hidden_is_an_unknown_argument_now_and_opens_a_window() {
        let mode = Launch::from_args(["--hidden"]);
        assert_eq!(mode, Launch::Ui);
        assert!(mode.creates_window());
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

    /// A stray `--hidden` beside `--daemon` cannot demote the daemon, in
    /// either order. Worth keeping after the removal rather than deleting
    /// with the flag: it is the shape a half-rewritten `Run` key would have.
    #[test]
    fn daemon_survives_a_leftover_hidden_in_either_order() {
        assert_eq!(Launch::from_args(["--hidden", "--daemon"]), Launch::Daemon);
        assert_eq!(Launch::from_args(["--daemon", "--hidden"]), Launch::Daemon);
    }

    #[test]
    fn unknown_arguments_are_ignored_rather_than_fatal() {
        // Both OSes hand launched apps arguments we never asked for — macOS
        // adds `-psn_...` on some launch paths, Windows passes shell verbs.
        assert_eq!(
            Launch::from_args(["-psn_0_12345", "--hidden", "/unexpected"]),
            Launch::Ui
        );
        assert_eq!(Launch::from_args(["--not-a-flag"]), Launch::Ui);
    }

    #[test]
    fn only_the_ui_mode_creates_a_window() {
        assert!(Launch::Ui.creates_window());
        assert!(!Launch::Daemon.creates_window());
        assert!(!Launch::CaptureWorker.creates_window());
    }

    /// The worker is its own mode, and nothing beside it can turn it into a
    /// daemon or a window: a worker that became a second daemon would find
    /// the endpoint owned and leave, and one that became a window would
    /// open one on the player's screen mid-game.
    #[test]
    fn the_capture_worker_is_its_own_mode_whatever_else_is_passed() {
        assert_eq!(Launch::from_args(["--capture-worker"]), Launch::CaptureWorker);
        assert_eq!(Launch::from_args(["--daemon", "--capture-worker"]), Launch::CaptureWorker);
        assert_eq!(Launch::from_args(["--capture-worker", "--daemon"]), Launch::CaptureWorker);
        assert_eq!(CAPTURE_WORKER_FLAG, "--capture-worker");
    }

    /// The registry holds the *string*, so renaming the constant is free and
    /// changing its value is not: an installed `HKCU\...\Run` entry written
    /// by an older build would stop meaning anything, and start-on-login would
    /// silently open a window instead of going to the tray.
    /// What login starts. Pinned because it is an on-disk contract: the value
    /// is written into the registry once and handed back years later, so
    /// changing it is a decision to make on purpose rather than a tidy-up.
    #[test]
    fn autostart_registers_the_daemon() {
        assert_eq!(autostart_args(), vec![DAEMON_FLAG]);
        // And whatever is registered has to parse back to a mode that opens no
        // window, or start-on-login becomes start-a-window-on-login.
        assert!(!Launch::from_args(autostart_args()).creates_window());
    }

    /// The other half of that contract. An entry written before WS3.5 no
    /// longer says anything this build understands, so it opens a window, and
    /// what stops that repeating every login is `daemon::autostart` rewriting
    /// the entry rather than anything here.
    #[test]
    fn an_entry_written_by_an_older_build_is_an_ordinary_start() {
        assert_eq!(Launch::from_args(["--hidden"]), Launch::Ui);
    }

    #[test]
    fn the_autostart_flag_is_the_exact_string_written_to_the_registry() {
        assert_eq!(DAEMON_FLAG, "--daemon");
        assert_eq!(Launch::from_args([DAEMON_FLAG]), Launch::Daemon);
    }
}
