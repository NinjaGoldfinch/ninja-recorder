//! Which capture backend the daemon builds: the `capture_backend` setting
//! (WS1.7, #11).
//!
//! Two backends sit behind `Recorder` for exactly one release: the own backend
//! (Option B, `recorder/own/`), which is the default since #243, and libobs,
//! which every earlier release recorded with. The plan keeps libobs
//! *selectable* for that release so a recording Option B gets wrong has
//! somewhere to go, and this module is the switch between them. WS8 deletes
//! it along with libobs.
//!
//! Three pieces, split the way `retention` and `state_machine` are:
//!
//! - [`resolve`] is pure: given the saved setting, if there is one, and what
//!   this build can offer, it says which backend to build or why it will not.
//!   [`choose`] is its half for a saved choice. Directly unit-tested.
//! - [`Backends`] is the I/O: what is available here, and constructing one.
//!   The daemon implements it, because only the daemon knows where the libobs
//!   worker is staged and only the daemon may own a `Recorder`.
//! - [`construct`] joins them, and is deliberately too small to hide a bug.
//!
//! **Nothing here fabricates a recording.** A backend the user *saved* that
//! cannot be built becomes a `FailedRecorder` carrying the reason, the same
//! refusal a missing libobs worker has always produced. It never falls back to
//! the other backend: the saved setting is the user's answer to "which one",
//! and a silent substitution is exactly the thing that makes a bad recording
//! impossible to attribute (`RecordingDiagnostics::backend`).
//!
//! **With nothing saved, the choice is ours**, and it is own where own can be
//! built and libobs otherwise (#243): nobody asked for own on a Windows 10
//! machine, so refusing to record there would be refusing on the user's
//! behalf. The fallback is logged with its reason and shown in Settings as
//! "Automatic", so it is attributable without being a refusal.
//!
//! See DEVELOPMENT.md §16, "The switch, and when it applies".

use super::{FailedRecorder, Recorder};

/// The two capture backends, as `settings_kv` stores them.
///
/// **The default is `Own`, since #243**, wherever own can be built; where it
/// cannot, a missing key resolves to libobs ([`resolve`]). The flip only
/// changed what a *missing* key means: someone who picked libobs explicitly
/// has a stored row and keeps it, with no migration (DEVELOPMENT.md §16, "The
/// switch, and when it applies"). Pinned by a test, so moving it again is a
/// deliberate change rather than an accident.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, ts_rs::TS,
)]
#[serde(rename_all = "lowercase")]
pub enum CaptureBackend {
    /// libobs through the patched fork's out-of-process worker. The fallback,
    /// selectable for one release after Option B ships.
    Libobs,
    /// Option B: WGC → D3D11 → Media Foundation, in `recorder/own/`, on
    /// Windows build 20348 or newer. The default.
    #[default]
    Own,
}

impl CaptureBackend {
    /// Parses the stored value: `None` for no row, and for anything this
    /// build does not recognise. Unrecognised is treated as unset rather than
    /// an error: `settings_kv` is shared across versions, and a downgrade must
    /// not stop the daemon from choosing a backend at all.
    pub fn from_pref(value: Option<&str>) -> Option<Self> {
        match value {
            Some("libobs") => Some(CaptureBackend::Libobs),
            Some("own") => Some(CaptureBackend::Own),
            _ => None,
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
    /// The saved choice, or, when nothing is saved, the backend [`resolve`]
    /// picked for this machine (own where it can be built, else libobs).
    pub configured: CaptureBackend,
    /// Nothing is saved: `configured` is the app's pick, not the user's. The
    /// row says "Automatic" and why, and only a click writes the setting.
    pub automatic: bool,
    /// What the live backend says it is (`Recorder::backend_name`), e.g.
    /// `libobs (ready)` or `unavailable (…)`. The two differ when the
    /// configured backend was refused, and this is how the row finds out.
    pub active: String,
    /// The live backend is encoding video in software
    /// (`Recorder::software_encoding`), so recording costs noticeably more
    /// CPU. The row shows a notice while it is true (DEVELOPMENT.md §2.4).
    pub software_encoding: bool,
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

/// Which backend to build for a **saved** `setting`, or why none will be.
///
/// Pure: the setting wins when it can be built, and is refused with the
/// option's own reason when it cannot. There is no fallback to the other
/// backend, on purpose (module header). [`resolve`] is the whole decision,
/// including what an unset key means.
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

/// The order an unset key tries the backends in: the default first.
const AUTOMATIC_ORDER: [CaptureBackend; 2] = [CaptureBackend::Own, CaptureBackend::Libobs];

/// What [`resolve`] decided to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub backend: CaptureBackend,
    /// Nothing was saved, so this is the app's pick.
    pub automatic: bool,
    /// For an automatic pick that is not the default: why the default could
    /// not be built. The daemon logs it once, at startup.
    pub fallback: Option<String>,
}

/// Which backend to build, from the saved setting (`None` when nothing is
/// saved) and what this build offers. Pure, and the whole of the decision:
///
/// - **saved**: exactly that backend, or refused with its reason ([`choose`]);
///   never the other one;
/// - **unset**: own if it can be built; otherwise libobs if *it* can, carrying
///   own's reason as the fallback; otherwise refused with both reasons.
pub fn resolve(
    saved: Option<CaptureBackend>,
    options: &[CaptureBackendOption],
) -> Result<Selection, String> {
    if let Some(setting) = saved {
        return choose(setting, options).map(|backend| Selection {
            backend,
            automatic: false,
            fallback: None,
        });
    }
    let mut refusals = Vec::new();
    for backend in AUTOMATIC_ORDER {
        match choose(backend, options) {
            Ok(backend) => {
                let fallback = (!refusals.is_empty()).then(|| refusals.join("; "));
                return Ok(Selection { backend, automatic: true, fallback });
            }
            Err(why) => refusals.push(format!("{} unavailable ({why})", backend.as_pref())),
        }
    }
    Err(format!("no capture backend can be built here: {}", refusals.join("; ")))
}

/// The backend `resolve` picks, or a `FailedRecorder` saying why not, and the
/// decision itself so the caller can log it.
pub fn construct(
    saved: Option<CaptureBackend>,
    backends: &dyn Backends,
) -> (Box<dyn Recorder>, Result<Selection, String>) {
    let decision = resolve(saved, &backends.options());
    let recorder: Box<dyn Recorder> = match &decision {
        Ok(selection) => backends.build(selection.backend),
        Err(why) => Box::new(FailedRecorder(why.clone())),
    };
    (recorder, decision)
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

    /// A Windows 11 machine: both can be built.
    fn windows_11() -> Vec<CaptureBackendOption> {
        vec![option(CaptureBackend::Libobs, None), option(CaptureBackend::Own, None)]
    }

    const NO_WORKER: &str = "the libobs worker is not beside the executable";

    /// Pinned so that moving the default is a deliberate change that fails
    /// this test, rather than something that happens by accident: where own
    /// can be built, nothing saved means own.
    #[test]
    fn the_default_is_own() {
        assert_eq!(CaptureBackend::default(), CaptureBackend::Own);
        assert_eq!(CaptureBackend::from_pref(None), None);
        let picked = resolve(None, &windows_11()).unwrap();
        assert_eq!(picked.backend, CaptureBackend::Own);
        assert!(picked.automatic);
        assert_eq!(picked.fallback, None);
    }

    /// The flip moves only the users who never chose. A stored `libobs` row
    /// is an explicit choice and keeps libobs, with no migration.
    #[test]
    fn a_stored_libobs_row_stays_on_libobs() {
        let saved = CaptureBackend::from_pref(Some("libobs"));
        assert_eq!(saved, Some(CaptureBackend::Libobs));
        let picked = resolve(saved, &windows_11()).unwrap();
        assert_eq!(picked.backend, CaptureBackend::Libobs);
        assert!(!picked.automatic);
    }

    #[test]
    fn the_stored_value_round_trips() {
        for backend in [CaptureBackend::Libobs, CaptureBackend::Own] {
            assert_eq!(CaptureBackend::from_pref(Some(backend.as_pref())), Some(backend));
        }
    }

    /// Every rule of `resolve`, as a table: what is saved, what the build
    /// offers, and what gets built (or the words the refusal must contain).
    #[test]
    fn resolve_follows_the_rules() {
        use CaptureBackend::{Libobs, Own};
        let both_down = vec![option(Libobs, Some(NO_WORKER)), option(Own, Some(TOO_OLD))];
        let own_only = vec![option(Libobs, Some(NO_WORKER)), option(Own, None)];
        type Want = Result<(CaptureBackend, bool, Option<String>), Vec<&'static str>>;
        let fell_back = format!("own unavailable ({TOO_OLD})");
        let cases: Vec<(&str, Option<CaptureBackend>, Vec<CaptureBackendOption>, Want)> = vec![
            ("unset, own buildable", None, windows_11(), Ok((Own, true, None))),
            ("unset, own buildable, libobs not", None, own_only.clone(), Ok((Own, true, None))),
            ("unset, own below its floor", None, windows_10(), Ok((Libobs, true, Some(fell_back)))),
            ("unset, neither", None, both_down.clone(), Err(vec![TOO_OLD, NO_WORKER])),
            ("saved own, buildable", Some(Own), windows_11(), Ok((Own, false, None))),
            ("saved own, below its floor", Some(Own), windows_10(), Err(vec![TOO_OLD])),
            ("saved libobs, buildable", Some(Libobs), windows_10(), Ok((Libobs, false, None))),
            ("saved libobs, no worker", Some(Libobs), own_only, Err(vec![NO_WORKER])),
            ("saved own, neither", Some(Own), both_down, Err(vec![TOO_OLD])),
        ];
        for (name, saved, options, want) in cases {
            let got = resolve(saved, &options);
            match (got, want) {
                (Ok(sel), Ok((backend, automatic, fallback))) => {
                    assert_eq!(sel, Selection { backend, automatic, fallback }, "{name}");
                }
                (Err(why), Err(words)) => {
                    for word in words {
                        assert!(why.contains(word), "{name}: {why:?} lacks {word:?}");
                    }
                }
                (got, want) => panic!("{name}: got {got:?}, want {want:?}"),
            }
        }
    }

    /// A saved choice is never substituted, even when the other one would
    /// record: that is the difference between saved and unset.
    #[test]
    fn a_saved_own_below_the_floor_is_refused_not_moved_to_libobs() {
        assert_eq!(resolve(Some(CaptureBackend::Own), &windows_10()), Err(TOO_OLD.to_string()));
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

    /// A downgrade reads whatever a newer build wrote, as unset.
    #[test]
    fn an_unrecognised_value_is_unset() {
        for raw in ["", "OWN", "mediafoundation", "{\"x\":1}"] {
            assert_eq!(CaptureBackend::from_pref(Some(raw)), None, "{raw:?}");
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

        let (refused, _) = construct(Some(CaptureBackend::Own), &Windows10);
        assert_eq!(refused.backend_name(), format!("unavailable ({TOO_OLD})"));
        assert!(!refused.is_recording());

        let (built, _) = construct(Some(CaptureBackend::Libobs), &Windows10);
        assert_eq!(built.backend_name(), "stub");

        // Unset on Windows 10: built, on libobs, and the decision says why.
        let (automatic, decision) = construct(None, &Windows10);
        assert_eq!(automatic.backend_name(), "stub");
        assert_eq!(decision.unwrap().backend, CaptureBackend::Libobs);
    }
}
