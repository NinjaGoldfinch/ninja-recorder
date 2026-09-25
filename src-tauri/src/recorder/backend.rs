//! Which capture backend the daemon builds: the `capture_backend` setting
//! (WS1.7, #11).
//!
//! Two backends sit behind `Recorder` for exactly one release: libobs, which
//! is what ships today, and the own backend (Option B, `recorder/own/`), which
//! is the target. The plan keeps libobs *selectable* for that release so a
//! recording Option B gets wrong has somewhere to go, and this module is the
//! switch between them. WS8 deletes it along with libobs.
//!
//! Three pieces, split the way `retention` and `state_machine` are:
//!
//! - [`choose`] is pure: given the setting and what this build can offer, it
//!   says which backend to build or why it will not. Directly unit-tested.
//! - [`Backends`] is the I/O: what is available here, and constructing one.
//!   The daemon implements it, because only the daemon knows where the libobs
//!   worker is staged and only the daemon may own a `Recorder`.
//! - [`construct`] joins them, and is deliberately too small to hide a bug.
//!
//! **Nothing here fabricates a recording.** A backend that is chosen but
//! cannot be built becomes a `FailedRecorder` carrying the reason, the same
//! refusal a missing libobs worker has always produced. It never falls back to
//! the other backend: the setting is the user's answer to "which one", and a
//! silent substitution is exactly the thing that makes a bad recording
//! impossible to attribute (`RecordingDiagnostics::backend`).
//!
//! See DEVELOPMENT.md §16, "The switch, and when it applies".

use super::{FailedRecorder, Recorder};

/// The two capture backends, as `settings_kv` stores them.
///
/// **The default is libobs, and WS1.6's last piece (#243) flips it to
/// `Own`.** `Own` is constructible since #236, records the Game audio
/// preset since #237 and every preset's sources, mixed into one track, since
/// #238; but until it writes every track through its own writer (#239) a
/// default of `Own` would make worse recordings for everyone who never
/// opened Settings. Flipping this `#[default]` is #243's change, not
/// a separate decision: the plan has Option B as the default once it is whole
/// (§4.5). A user who picked libobs explicitly keeps it across that flip,
/// because the flip only changes what a *missing* key means. The same change
/// un-hides the Settings row, which is devtools-only until then
/// (`Settings.svelte`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
pub enum CaptureBackend {
    /// libobs through the patched fork's out-of-process worker. The fallback,
    /// and selectable for one release after Option B ships.
    #[default]
    Libobs,
    /// Option B: WGC → D3D11 → Media Foundation, in `recorder/own/`.
    /// Constructible on Windows build 20348 or newer since #236; every audio
    /// preset since #238, as one mixed track until the stems arrive (#239).
    Own,
}

impl CaptureBackend {
    /// Parses the stored value. Anything unrecognised is the default rather
    /// than an error: `settings_kv` is shared across versions, and a
    /// downgrade must not stop the daemon from choosing a backend at all.
    pub fn from_pref(value: Option<&str>) -> Self {
        match value {
            Some("libobs") => CaptureBackend::Libobs,
            Some("own") => CaptureBackend::Own,
            _ => CaptureBackend::default(),
        }
    }

    /// The string written to `settings_kv`; round-trips `from_pref`, and is
    /// the same spelling serde uses on the wire.
    pub fn as_pref(self) -> &'static str {
        match self {
            CaptureBackend::Libobs => "libobs",
            CaptureBackend::Own => "own",
        }
    }
}

/// One backend, and whether this build can construct it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct CaptureBackendOption {
    pub backend: CaptureBackend,
    /// Why it cannot be built here, or `null` when it can. A reason rather
    /// than a flag, because the settings row shows it beside the disabled
    /// choice and a bare "unavailable" gives nobody anything to act on.
    pub unavailable: Option<String>,
}

/// What the settings row renders.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
pub struct CaptureBackendStatus {
    /// The saved choice, or the default if none was ever saved.
    pub configured: CaptureBackend,
    /// What the live backend says it is (`Recorder::backend_name`), e.g.
    /// `libobs (ready)` or `unavailable (…)`. The two differ when the
    /// configured backend was refused, and this is how the row finds out.
    pub active: String,
    /// Every backend this build knows about, available or not, in the order
    /// the control lists them.
    pub options: Vec<CaptureBackendOption>,
}

/// What this process can offer, and how to build it.
///
/// A trait for the reason `core::Autostart` is one: the only real
/// implementation resolves paths the daemon owns, and `core` must stay
/// testable without them. `None` in `Ctx` means the process does not own the
/// recorder, which is the UI and every unit test.
pub trait Backends: Send + Sync {
    /// Every backend, whether or not it can be built. Checked on each call
    /// rather than once at startup: it is a file-exists test, and a worker
    /// restored by a repair install should not need a daemon restart to be
    /// offered again.
    fn options(&self) -> Vec<CaptureBackendOption>;

    /// Constructs `backend`. Only called with what [`choose`] allowed, so an
    /// implementation may treat an unavailable one as a refusal rather than
    /// handling it twice.
    fn build(&self, backend: CaptureBackend) -> Box<dyn Recorder>;
}

/// Which backend to build for `setting`, or why none will be.
///
/// Pure, and the whole of the decision: the setting wins when it can be
/// built, and is refused with the option's own reason when it cannot. There
/// is no fallback to the other backend, on purpose (module header).
pub fn choose(
    setting: CaptureBackend,
    options: &[CaptureBackendOption],
) -> Result<CaptureBackend, String> {
    match options.iter().find(|o| o.backend == setting) {
        Some(CaptureBackendOption { unavailable: None, .. }) => Ok(setting),
        Some(CaptureBackendOption { unavailable: Some(why), .. }) => Err(why.clone()),
        None => Err(format!("the {} capture backend is not in this build", setting.as_pref())),
    }
}

/// The backend `setting` asks for, or a `FailedRecorder` saying why not.
pub fn construct(setting: CaptureBackend, backends: &dyn Backends) -> Box<dyn Recorder> {
    match choose(setting, &backends.options()) {
        Ok(backend) => backends.build(backend),
        Err(why) => Box::new(FailedRecorder(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(backend: CaptureBackend, unavailable: Option<&str>) -> CaptureBackendOption {
        CaptureBackendOption { backend, unavailable: unavailable.map(str::to_string) }
    }

    /// Why the own backend is refused below its OS floor, in the shape
    /// `select::availability` words it.
    const TOO_OLD: &str = "the own capture backend needs Windows build 20348 or newer";

    /// A Windows 10 machine: libobs works, the own backend is below its floor.
    fn windows_10() -> Vec<CaptureBackendOption> {
        vec![option(CaptureBackend::Libobs, None), option(CaptureBackend::Own, Some(TOO_OLD))]
    }

    /// Pinned so that #243's flip is a deliberate one-line change that fails
    /// this test, rather than something that happens by accident.
    #[test]
    fn the_default_is_libobs_until_243_flips_it() {
        assert_eq!(CaptureBackend::default(), CaptureBackend::Libobs);
        assert_eq!(CaptureBackend::from_pref(None), CaptureBackend::Libobs);
    }

    #[test]
    fn the_stored_value_round_trips() {
        for backend in [CaptureBackend::Libobs, CaptureBackend::Own] {
            assert_eq!(CaptureBackend::from_pref(Some(backend.as_pref())), backend);
        }
    }

    /// The pref spelling and the wire spelling are the same string, so the
    /// frontend can compare what it sent with what it reads back.
    #[test]
    fn the_wire_and_the_pref_agree() {
        for backend in [CaptureBackend::Libobs, CaptureBackend::Own] {
            let json = serde_json::to_string(&backend).unwrap();
            assert_eq!(json, format!("\"{}\"", backend.as_pref()));
        }
    }

    /// A downgrade reads whatever a newer build wrote.
    #[test]
    fn an_unrecognised_value_is_the_default() {
        for raw in ["", "OWN", "mediafoundation", "{\"x\":1}"] {
            assert_eq!(CaptureBackend::from_pref(Some(raw)), CaptureBackend::default(), "{raw:?}");
        }
    }

    #[test]
    fn an_available_setting_is_built() {
        assert_eq!(choose(CaptureBackend::Libobs, &windows_10()), Ok(CaptureBackend::Libobs));
    }

    /// Choosing the own backend where it cannot run refuses with the reason,
    /// and does not quietly record on libobs.
    #[test]
    fn an_unavailable_setting_is_refused_with_its_reason_not_substituted() {
        assert_eq!(choose(CaptureBackend::Own, &windows_10()), Err(TOO_OLD.to_string()));
    }

    #[test]
    fn a_missing_libobs_worker_is_refused_the_same_way() {
        let options = vec![
            option(CaptureBackend::Libobs, Some("the libobs worker is not beside the executable")),
            option(CaptureBackend::Own, None),
        ];
        assert_eq!(
            choose(CaptureBackend::Libobs, &options),
            Err("the libobs worker is not beside the executable".to_string())
        );
    }

    /// A build on a new enough Windows, since #236.
    #[test]
    fn the_own_backend_is_built_once_it_is_available() {
        let options = vec![option(CaptureBackend::Libobs, None), option(CaptureBackend::Own, None)];
        assert_eq!(choose(CaptureBackend::Own, &options), Ok(CaptureBackend::Own));
    }

    #[test]
    fn a_backend_the_build_does_not_list_is_refused() {
        let options = vec![option(CaptureBackend::Libobs, None)];
        let err = choose(CaptureBackend::Own, &options).unwrap_err();
        assert!(err.contains("not in this build"), "{err}");
    }

    /// `construct` is the thin wrapper, so the one thing worth pinning is that
    /// a refusal reaches the recorder as a refusal.
    #[test]
    fn construct_turns_a_refusal_into_a_failed_recorder() {
        struct Windows10;
        impl Backends for Windows10 {
            fn options(&self) -> Vec<CaptureBackendOption> {
                windows_10()
            }
            fn build(&self, _backend: CaptureBackend) -> Box<dyn Recorder> {
                Box::new(crate::recorder::stub::StubRecorder::new())
            }
        }

        let refused = construct(CaptureBackend::Own, &Windows10);
        assert_eq!(refused.backend_name(), format!("unavailable ({TOO_OLD})"));
        assert!(!refused.is_recording());

        let built = construct(CaptureBackend::Libobs, &Windows10);
        assert_eq!(built.backend_name(), "stub");
    }
}
