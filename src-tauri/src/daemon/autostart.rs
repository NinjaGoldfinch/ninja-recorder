//! Start-on-login, from the process that login starts. #151.
//!
//! `Ctx` carries an autostart seam and until now exactly one place filled it:
//! `lib.rs`, in the UI. WS3.4 moved every command into the daemon, so
//! `get_autostart` and `set_autostart` ran in a process whose seam was never
//! set, and the settings row said "start-on-login is not available in this
//! build" on a build where it was. The feature had been dead since that move
//! and said so politely enough that nobody noticed.
//!
//! ## Why the daemon and not the window
//!
//! §3.1's ownership table puts the Run key with the daemon, alongside the tray
//! and the updater, and that is right rather than arbitrary: the daemon is the
//! process login starts. `launch::autostart_args()` returns `--daemon` for
//! exactly that reason, and a window that owned this would be writing an entry
//! describing a process it is not.
//!
//! ## Why not `tauri-plugin-autostart`
//!
//! Its API hangs off an `AppHandle` and the daemon builds no Tauri app. Same
//! wall `daemon::Paths` and `daemon::notify` hit, and the same way through it:
//! use the crate the plugin wraps. `auto-launch` was already in the tree
//! through that plugin, so naming it directly adds no crate and no licence.
//!
//! It also keeps the behaviour the plugin had rather than reimplementing it.
//! In particular `is_enabled` consults Task Manager's own
//! `StartupApproved\Run` override, so an entry that exists but has been
//! switched off from the Startup tab reads as disabled, which is what
//! DEVELOPMENT.md §12 means by the registry being the source of truth.

use auto_launch::{AutoLaunch, AutoLaunchBuilder};

use crate::core::Autostart;
use crate::launch;
use crate::{info, warn};

/// The name the entry is filed under in `HKCU\...\Run`.
///
/// **An on-disk contract, and not ours to choose freely.**
/// `tauri-plugin-autostart` defaulted `app_name` to
/// `app.package_info().name`, which Tauri fills from `productName`. Every user
/// who turned start-on-login on before this change has an entry under that
/// exact string, and `AutoLaunch::is_enabled` looks a value up by name without
/// checking the path, so matching it is the whole of what makes an old entry
/// visible to this code.
///
/// Scoped by build for the same reason the endpoint is: the devtools bundle
/// overrides `productName`, installs beside the release build and would
/// otherwise share one login entry with it, so whichever was toggled last
/// would decide what login started.
const APP_NAME: &str = if cfg!(feature = "devtools") {
    "ninja-recorder-dev"
} else {
    "ninja-recorder"
};

/// The daemon's `Autostart`, over the registry entry Windows actually reads.
pub struct RegistryAutostart(AutoLaunch);

impl RegistryAutostart {
    /// Resolves the executable to register and builds the handle.
    ///
    /// `current_exe` rather than a configured path: one binary serves both
    /// roles, so the daemon writing its own path is the same path a window
    /// would have written, and an app that has been moved registers where it
    /// now is rather than where it was installed.
    pub fn resolve() -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot find this executable to register it: {e}"))?;
        let exe = exe
            .to_str()
            .ok_or("this executable's path is not valid UTF-8, so it cannot be registered")?;

        AutoLaunchBuilder::new()
            .set_app_name(APP_NAME)
            .set_app_path(exe)
            .set_args(&launch::autostart_args())
            .build()
            .map(RegistryAutostart)
            .map_err(|e| format!("cannot prepare the login entry: {e}"))
    }
}

impl RegistryAutostart {
    /// Rewrites an enabled login entry so it holds the current arguments.
    ///
    /// **This is what makes removing `--hidden` cost one window rather than
    /// one per login** (#71). An entry written before WS3.5 still says
    /// `--hidden`, which this build does not recognise, so Windows starts an
    /// ordinary window at login. That window starts a daemon (`daemon::spawn`)
    /// and this runs, so the entry is corrected on the same login that showed
    /// the window and the next one is a daemon start.
    ///
    /// `enable()` on an already-enabled entry is a plain overwrite of the
    /// value under `APP_NAME`, which is why this needs no comparison and no
    /// reading of what is there: writing the right answer is cheaper than
    /// working out whether it is already the right answer, and it is correct
    /// whatever the entry said.
    ///
    /// Only when it is **already enabled**. Start-on-login is the user's
    /// choice and an app that turned it on by itself would be doing something
    /// nobody asked for; this only ever changes what an entry says, never
    /// whether one exists.
    pub fn refresh_if_enabled(&self) {
        match self.0.is_enabled() {
            Ok(true) => match self.0.enable() {
                Ok(()) => info!("autostart", "login entry rewritten with the current arguments"),
                // Not fatal: the entry keeps whatever it had, which at worst
                // is the old flag, and the only cost is the window appearing
                // again at the next login.
                Err(e) => warn!("autostart", "could not rewrite the login entry: {e}"),
            },
            Ok(false) => {}
            Err(e) => warn!("autostart", "could not read the login entry: {e}"),
        }
    }
}

impl Autostart for RegistryAutostart {
    fn is_enabled(&self) -> Result<bool, String> {
        self.0.is_enabled().map_err(|e| e.to_string())
    }

    fn enable(&self) -> Result<(), String> {
        self.0.enable().map_err(|e| e.to_string())
    }

    fn disable(&self) -> Result<(), String> {
        self.0.disable().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name is what makes an entry written by an older build findable, so
    /// it is pinned against the config Tauri would have taken it from rather
    /// than left as a literal someone could tidy.
    #[test]
    fn the_app_name_is_the_product_name_tauri_would_have_used() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let devtools: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.devtools.conf.json")).unwrap();

        let expected = if cfg!(feature = "devtools") {
            devtools["productName"].as_str()
        } else {
            conf["productName"].as_str()
        };

        assert_eq!(
            Some(APP_NAME),
            expected,
            "the login entry's name must match the product name, or an entry \
             written before #151 becomes invisible"
        );
    }

    /// The two builds install side by side and share a data directory. They
    /// must not share a login entry, or whichever was toggled last decides
    /// what login starts.
    #[test]
    fn the_two_builds_do_not_share_one_entry() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let devtools: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.devtools.conf.json")).unwrap();

        assert_ne!(conf["productName"], devtools["productName"]);
    }

    /// What gets written after the path. Pinned here as well as in `launch`,
    /// because this is the code that writes it and the two must not drift.
    #[test]
    fn the_entry_registers_the_daemon() {
        assert_eq!(launch::autostart_args(), vec![launch::DAEMON_FLAG]);
    }

    /// And what it writes has to parse back to the daemon, which is the claim
    /// `refresh_if_enabled` rests on: it overwrites an old entry with these
    /// arguments, so if they did not round-trip it would replace one broken
    /// login entry with another.
    ///
    /// The rewrite itself is not unit tested, because it writes to the real
    /// `HKCU\...\Run` of whoever runs the suite. `windows-verification.md`
    /// §5.0.2 is where it is checked.
    #[test]
    fn what_the_rewrite_writes_parses_back_to_a_daemon_start() {
        let mode = launch::Launch::from_args(launch::autostart_args());
        assert_eq!(mode, launch::Launch::Daemon);
        assert!(!mode.creates_window(), "a login start must not open a window");
    }
}
