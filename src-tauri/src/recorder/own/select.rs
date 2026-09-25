//! Which H.264 encoder to use, and whether this Windows can run the own
//! backend at all, with no Windows in it.
//!
//! `own/win/` enumerates the DXGI adapters and Media Foundation's H.264
//! encoders (`spikes/p0c-video/src/win/` shows how) and hands plain
//! descriptions of them to [`rank`]. The decision is here so every case,
//! including the machines nobody has on a desk, is a unit test.
//!
//! The policy is DEVELOPMENT.md §2.4's: hardware first, by adapter vendor,
//! NVIDIA → AMD → Intel; Microsoft's software H.264 MFT only when no usable
//! hardware encoder exists, and then as a [`Choice::SoftwareFallback`] that
//! says why, so no caller can use it without knowing. [`choose`] is what the
//! session calls: [`rank`], or the software MFT when a devtools build is told
//! to take the fallback on purpose ([`FORCE_SOFTWARE_ENV`]).
//!
//! It also answers the two other "can it record this" questions: the Windows
//! build floor ([`availability`]), and whether an audio preset is a layout at
//! all ([`audio_layout`]).

use crate::recorder::audio::{AudioLayout, AudioPreset};

/// PCI vendor ids, as DXGI reports them and as `VEN_xxxx` in an MFT's
/// `MFT_ENUM_HARDWARE_VENDOR_ID_Attribute`.
pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

/// The order hardware encoders are preferred in. A hybrid laptop lists its
/// iGPU first; the discrete GPU still wins, because it is the one the game
/// is most likely rendering on and its encoder is the stronger one.
const PREFERENCE: [u32; 3] = [VENDOR_NVIDIA, VENDOR_AMD, VENDOR_INTEL];

/// The PCI vendor id in a `VEN_xxxx` string, or `None` if it does not start
/// with four hex digits. The `VEN_` prefix is optional.
pub fn vendor_id(vendor: &str) -> Option<u32> {
    let hex = vendor.strip_prefix("VEN_").unwrap_or(vendor);
    u32::from_str_radix(hex.get(..4)?, 16).ok()
}

/// A name for a vendor id, for log lines and the notice.
pub fn vendor_name(id: u32) -> &'static str {
    match id {
        VENDOR_NVIDIA => "NVIDIA",
        VENDOR_AMD => "AMD",
        VENDOR_INTEL => "Intel",
        0x1414 => "Microsoft",
        _ => "unknown vendor",
    }
}

/// One DXGI adapter, as `own/win/` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
    /// `DXGI_ADAPTER_FLAG_SOFTWARE`: the Basic Render Driver, which has no
    /// encoder behind it whatever its vendor id says.
    pub software: bool,
}

/// One H.264 encoder MFT, as `own/win/` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Encoder {
    pub name: String,
    /// The `VEN_xxxx` string a hardware MFT carries; `None` for software.
    pub vendor: Option<String>,
    /// Enumerated with `MFT_ENUM_FLAG_HARDWARE`.
    pub hardware: bool,
}

/// What [`rank`] decided. Indices point into the slices it was given, so the
/// caller keeps its COM objects alongside and nothing here has to hold them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A hardware encoder whose vendor matches a hardware adapter.
    Hardware { encoder: usize, adapter: usize },
    /// Microsoft's software H.264 MFT, because nothing better is usable.
    /// Never silent: the reason goes to `daemon.log`, `diagnostics_json` and
    /// a UI notice about the extra CPU (DEVELOPMENT.md §2.4).
    SoftwareFallback { encoder: usize, reason: String },
    /// Nothing to encode with at all. The backend refuses to start.
    Unavailable { reason: String },
}

impl Choice {
    /// True for the software fallback, which every caller must surface.
    pub fn is_fallback(&self) -> bool {
        matches!(self, Choice::SoftwareFallback { .. })
    }
}

/// Pick the encoder, hardware first.
///
/// For each vendor in NVIDIA → AMD → Intel order, the first hardware adapter
/// of that vendor is paired with the first hardware encoder carrying the same
/// vendor id, in Media Foundation's own order. An encoder whose vendor string
/// does not parse, or that matches no hardware adapter, is not usable: an
/// encoder on a GPU the capture is not on would need a cross-adapter copy per
/// frame. Only when no pair exists is a software encoder chosen, and then as
/// a fallback with the reason.
pub fn rank(adapters: &[Adapter], encoders: &[Encoder]) -> Choice {
    for vendor in PREFERENCE {
        let Some(adapter) = adapters.iter().position(|a| !a.software && a.vendor == vendor) else {
            continue;
        };
        let encoder = encoders
            .iter()
            .position(|e| e.hardware && e.vendor.as_deref().and_then(vendor_id) == Some(vendor));
        if let Some(encoder) = encoder {
            return Choice::Hardware { encoder, adapter };
        }
    }

    let reason = no_hardware_reason(adapters, encoders);
    match encoders.iter().position(|e| !e.hardware) {
        Some(encoder) => Choice::SoftwareFallback { encoder, reason },
        None => Choice::Unavailable {
            reason: format!("{reason}, and Media Foundation offers no software H.264 encoder"),
        },
    }
}

/// Why no hardware pair was found, in the words the notice and the log use.
fn no_hardware_reason(adapters: &[Adapter], encoders: &[Encoder]) -> String {
    let gpus: Vec<&Adapter> = adapters.iter().filter(|a| !a.software).collect();
    let hardware: Vec<&Encoder> = encoders.iter().filter(|e| e.hardware).collect();
    if gpus.is_empty() {
        return "no hardware GPU was found".to_string();
    }
    if hardware.is_empty() {
        let names: Vec<&str> = gpus.iter().map(|a| a.name.as_str()).collect();
        return format!("no hardware H.264 encoder is installed for {}", names.join(", "));
    }
    let offered: Vec<String> = hardware
        .iter()
        .map(|e| match e.vendor.as_deref() {
            Some(v) => format!("{} ({v})", e.name),
            None => format!("{} (no vendor id)", e.name),
        })
        .collect();
    let gpus: Vec<String> = gpus
        .iter()
        .map(|a| format!("{} ({})", a.name, vendor_name(a.vendor)))
        .collect();
    format!(
        "no hardware H.264 encoder matches a supported GPU: GPUs {}; encoders {}",
        gpus.join(", "),
        offered.join(", ")
    )
}

/// The oldest Windows build the own backend runs on: **19041**, Windows 10
/// 2004. That takes in every Windows 10 still in service (2004 to 22H2,
/// builds 19041 to 19045) as well as Windows 11.
///
/// Set by the stricter of the two APIs it cannot do without:
///
/// - **Process loopback** (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`),
///   for game audio on its own track. Microsoft documents it from build
///   20348: "Minimum supported client: Windows 10 Build 20348", per
///   Microsoft Learn's `AUDIOCLIENT_ACTIVATION_TYPE` page
///   (<https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ne-audioclientactivationparams-audioclient_activation_type>),
///   and the ApplicationLoopback sample's README says the same. **OBS
///   enables it from 19041**, earlier than Microsoft documents, and that is
///   the floor used here.
/// - **WGC window capture** (`IGraphicsCaptureItemInterop::CreateForWindow`):
///   Windows 10 1903, build 18362, per its Microsoft Learn page, so below
///   either answer.
///
/// **Following OBS is unverified on Windows 10 hardware.** #237's decision
/// (<https://github.com/NinjaGoldfinch/ninja-recorder/issues/237#issuecomment-5822380979>)
/// was to lower the floor from 20348 only once a Windows 10 box had shown
/// process loopback working there. #291 lowered it without that run, on
/// the owner's decision that a bug report can confirm it instead. What makes
/// that acceptable is that the failure is handled rather than silent: an
/// activation Windows refuses costs the game's audio, not the recording
/// (`plan::realised_layout`), and since #10 it is shown to the user, as a
/// desktop notification, a strip in the window and a line on the recording,
/// naming the Windows build, the failing call and its HRESULT
/// (`own::problem`, `recorder::problem`). The run in
/// `spikes/p0c-audio/README.md` is now optional confirmation rather than a
/// gate.
pub const MIN_BUILD: u32 = 19_041;

/// `None` if the own backend can run on Windows build `build`, or the reason
/// it cannot, for the backend chooser to show.
pub fn availability(build: u32) -> Option<String> {
    (build < MIN_BUILD).then(|| {
        format!(
            "the own capture backend needs Windows build {MIN_BUILD} or newer (Windows 10 \
             version 2004 or later), for per-application audio capture; this is build {build}"
        )
    })
}

/// The environment variable that lets a **devtools** build construct the own
/// backend below [`MIN_BUILD`]. It was for trying Windows 10 at all before the
/// floor came down to 19041 (#237); what is left below the floor now is
/// Windows 10 1903 and 1909 (builds 18362 and 18363), where WGC window capture
/// exists and process loopback is not expected to, so this is for finding out
/// how far the backend gets there. A release build never reads it.
pub const IGNORE_FLOOR_ENV: &str = "NINJA_OWN_IGNORE_OS_FLOOR";

/// Whether the floor override is in force: only in a devtools build, and
/// only for the value `1`. Pure, so the release half is a test; the daemon
/// passes `cfg!(feature = "devtools")` and the variable's value.
pub fn floor_ignored(devtools: bool, value: Option<&str>) -> bool {
    devtools && value == Some("1")
}

/// The environment variable that makes a **devtools** build encode with
/// Microsoft's software H.264 MFT even when a hardware encoder is there, so
/// the fallback path (the log line, `backend_name`, the stop summary, the
/// diagnostics, the CPU it costs) can be exercised on a machine that would
/// never take it. A release build never reads it.
pub const FORCE_SOFTWARE_ENV: &str = "NINJA_OWN_FORCE_SOFTWARE_ENCODER";

/// The reason a forced fallback carries, everywhere a real one's goes.
pub const FORCED_SOFTWARE_REASON: &str = "forced by NINJA_OWN_FORCE_SOFTWARE_ENCODER (devtools)";

/// Whether the software-encoder override is in force: only in a devtools
/// build, and only for the value `1`. Pure, like [`floor_ignored`].
pub fn software_forced(devtools: bool, value: Option<&str>) -> bool {
    devtools && value == Some("1")
}

/// [`software_forced`] for this build, reading the variable through `lookup`
/// (`std::env::var` in production). A build without `devtools` returns
/// before `lookup` is called, so it never reads the variable at all.
pub fn software_forced_in_this_build(lookup: impl FnOnce(&str) -> Option<String>) -> bool {
    if !cfg!(feature = "devtools") {
        return false;
    }
    software_forced(true, lookup(FORCE_SOFTWARE_ENV).as_deref())
}

/// [`rank`], unless `force_software` is set: then Microsoft's software H.264
/// MFT, as a [`Choice::SoftwareFallback`] whose reason is
/// [`FORCED_SOFTWARE_REASON`], so everything downstream treats it exactly as
/// it treats a real fallback. With no software encoder to force, the backend
/// is unavailable and says why, rather than quietly using the hardware.
pub fn choose(adapters: &[Adapter], encoders: &[Encoder], force_software: bool) -> Choice {
    if !force_software {
        return rank(adapters, encoders);
    }
    match encoders.iter().position(|e| !e.hardware) {
        Some(encoder) => Choice::SoftwareFallback { encoder, reason: FORCED_SOFTWARE_REASON.to_string() },
        None => Choice::Unavailable {
            reason: format!("{FORCED_SOFTWARE_REASON}, but Media Foundation offers no software H.264 encoder"),
        },
    }
}

/// The audio layout the own backend records for `preset`, or why it cannot.
///
/// **Every preset, since #238.** Each source the layout names is captured by
/// its own thread (the game and applications by process loopback, the
/// microphone and the desktop from their endpoints), and all of them are
/// mixed into track 0. The stems after it arrive in #239: until then the sink
/// writer's one audio stream holds the mix only, and `own::plan` is what
/// narrows the layout to the tracks written. The only refusal left is a
/// layout that is not one at all, which only a hand-edited `Custom` preset can
/// be.
pub fn audio_layout(preset: &AudioPreset) -> Result<AudioLayout, String> {
    let layout = preset.layout();
    layout.validate().map_err(|e| format!("the audio preset cannot be recorded: {e}"))?;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::audio::{AudioSourceKind, AudioTrackSpec};

    fn gpu(name: &str, vendor: u32) -> Adapter {
        Adapter { name: name.to_string(), vendor, software: false }
    }

    fn basic_render() -> Adapter {
        Adapter { name: "Microsoft Basic Render Driver".to_string(), vendor: 0x1414, software: true }
    }

    fn hw(name: &str, vendor: &str) -> Encoder {
        Encoder { name: name.to_string(), vendor: Some(vendor.to_string()), hardware: true }
    }

    fn software() -> Encoder {
        Encoder { name: "H264 Encoder MFT".to_string(), vendor: None, hardware: false }
    }

    #[test]
    fn vendor_ids_parse_with_or_without_the_prefix() {
        assert_eq!(vendor_id("VEN_10DE"), Some(VENDOR_NVIDIA));
        assert_eq!(vendor_id("VEN_1002&DEV_73BF"), Some(VENDOR_AMD));
        assert_eq!(vendor_id("8086"), Some(VENDOR_INTEL));
        assert_eq!(vendor_id("VEN_"), None);
        assert_eq!(vendor_id("VEN_XYZW"), None);
        assert_eq!(vendor_id(""), None);
    }

    #[test]
    fn nvidia_only() {
        let choice = rank(&[gpu("RTX", VENDOR_NVIDIA)], &[hw("NVIDIA H.264 Encoder MFT", "VEN_10DE")]);
        assert_eq!(choice, Choice::Hardware { encoder: 0, adapter: 0 });
        assert!(!choice.is_fallback());
    }

    #[test]
    fn nvidia_and_software_prefers_the_hardware() {
        let choice = rank(
            &[gpu("RTX", VENDOR_NVIDIA), basic_render()],
            &[software(), hw("NVIDIA H.264 Encoder MFT", "VEN_10DE")],
        );
        assert_eq!(choice, Choice::Hardware { encoder: 1, adapter: 0 });
    }

    #[test]
    fn software_only_is_a_marked_fallback() {
        let choice = rank(&[gpu("Some GPU", 0x1234), basic_render()], &[software()]);
        let Choice::SoftwareFallback { encoder, reason } = &choice else {
            panic!("{choice:?}");
        };
        assert_eq!(*encoder, 0);
        assert!(reason.contains("no hardware H.264 encoder is installed"), "{reason}");
        assert!(choice.is_fallback());
    }

    #[test]
    fn no_gpu_at_all_falls_back_and_says_so() {
        let choice = rank(&[basic_render()], &[software()]);
        assert_eq!(
            choice,
            Choice::SoftwareFallback { encoder: 0, reason: "no hardware GPU was found".to_string() }
        );
    }

    #[test]
    fn nothing_to_encode_with_is_unavailable() {
        let choice = rank(&[basic_render()], &[]);
        assert!(matches!(choice, Choice::Unavailable { .. }), "{choice:?}");
        assert!(!choice.is_fallback());
    }

    #[test]
    fn a_hybrid_laptop_with_its_igpu_first_uses_the_discrete_gpu() {
        let choice = rank(
            &[gpu("Intel UHD", VENDOR_INTEL), gpu("RTX", VENDOR_NVIDIA)],
            &[hw("Intel Quick Sync H.264 Encoder MFT", "VEN_8086"), hw("NVIDIA H.264 Encoder MFT", "VEN_10DE")],
        );
        assert_eq!(choice, Choice::Hardware { encoder: 1, adapter: 1 });
    }

    #[test]
    fn amd_is_preferred_over_intel() {
        let choice = rank(
            &[gpu("Intel UHD", VENDOR_INTEL), gpu("Radeon", VENDOR_AMD)],
            &[hw("Intel Quick Sync H.264 Encoder MFT", "VEN_8086"), hw("AMDh264Encoder", "VEN_1002")],
        );
        assert_eq!(choice, Choice::Hardware { encoder: 1, adapter: 1 });
    }

    #[test]
    fn intel_alone_is_used() {
        let choice = rank(&[gpu("Intel UHD", VENDOR_INTEL)], &[software(), hw("Intel Quick Sync H.264 Encoder MFT", "VEN_8086")]);
        assert_eq!(choice, Choice::Hardware { encoder: 1, adapter: 0 });
    }

    #[test]
    fn an_unparseable_vendor_string_is_not_usable() {
        let choice = rank(&[gpu("RTX", VENDOR_NVIDIA)], &[hw("Mystery MFT", "NVIDIA"), software()]);
        let Choice::SoftwareFallback { encoder, reason } = &choice else {
            panic!("{choice:?}");
        };
        assert_eq!(*encoder, 1);
        assert!(reason.contains("Mystery MFT (NVIDIA)"), "{reason}");
    }

    #[test]
    fn an_encoder_for_a_gpu_that_is_not_there_is_not_usable() {
        // #224's shape: the encoder's vendor matches no adapter. That degrades
        // to software rather than refusing to record.
        let choice = rank(&[gpu("Radeon", VENDOR_AMD)], &[hw("NVIDIA H.264 Encoder MFT", "VEN_10DE"), software()]);
        assert!(choice.is_fallback(), "{choice:?}");
    }

    #[test]
    fn a_software_adapter_does_not_count_as_a_gpu() {
        // The Basic Render Driver with a spoofed vendor must not pair.
        let adapter = Adapter { software: true, ..gpu("Basic", VENDOR_NVIDIA) };
        let choice = rank(&[adapter], &[hw("NVIDIA H.264 Encoder MFT", "VEN_10DE"), software()]);
        assert!(choice.is_fallback(), "{choice:?}");
    }

    #[test]
    fn the_first_matching_encoder_in_media_foundations_order_wins() {
        let choice = rank(
            &[gpu("RTX", VENDOR_NVIDIA)],
            &[hw("NVIDIA first", "VEN_10DE"), hw("NVIDIA second", "VEN_10DE")],
        );
        assert_eq!(choice, Choice::Hardware { encoder: 0, adapter: 0 });
    }

    /// Pins the floor. Changing it is a decision with a source behind it
    /// (see `MIN_BUILD`), not a tidy-up: 19041 is OBS's floor, taken without
    /// a Windows 10 run (#237).
    #[test]
    fn the_os_floor_is_build_19041() {
        assert_eq!(MIN_BUILD, 19_041);
        for (build, runs) in [
            (18_362, false), // 1903: WGC window capture, no process loopback
            (18_363, false), // 1909
            (19_040, false),
            (19_041, true),  // 2004: where OBS enables process loopback
            (19_045, true),  // Windows 10 22H2, the last Windows 10
            (20_348, true),  // Server 2022, Microsoft's documented floor
            (22_000, true),  // Windows 11 21H2
            (26_200, true),  // the verification box (DEVELOPMENT.md §16)
        ] {
            assert_eq!(availability(build).is_none(), runs, "build {build}");
        }
        let reason = availability(18_363).unwrap();
        assert!(reason.contains("19041") && reason.contains("18363"), "{reason}");
        assert!(!reason.contains("Windows 11"), "Windows 10 2004 is enough: {reason}");
    }

    #[test]
    fn the_floor_override_is_devtools_only_and_needs_exactly_1() {
        assert_eq!(IGNORE_FLOOR_ENV, "NINJA_OWN_IGNORE_OS_FLOOR");
        assert!(floor_ignored(true, Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!floor_ignored(true, value), "{value:?}");
        }
        assert!(!floor_ignored(false, Some("1")), "a release build never honours it");
    }

    #[test]
    fn the_software_override_is_devtools_only_and_needs_exactly_1() {
        assert_eq!(FORCE_SOFTWARE_ENV, "NINJA_OWN_FORCE_SOFTWARE_ENCODER");
        assert!(software_forced(true, Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("yes"), Some(" 1")] {
            assert!(!software_forced(true, value), "{value:?}");
        }
        assert!(!software_forced(false, Some("1")), "a release build never honours it");
    }

    /// In this build, the override is honoured exactly when `devtools` is on:
    /// `cargo test` checks the release half, `cargo test --features devtools`
    /// the other.
    #[test]
    fn this_build_honours_the_software_override_only_with_devtools() {
        let set = |name: &str| (name == FORCE_SOFTWARE_ENV).then(|| "1".to_string());
        assert_eq!(software_forced_in_this_build(set), cfg!(feature = "devtools"));
        assert!(!software_forced_in_this_build(|_| None));
        assert!(!software_forced_in_this_build(|_| Some("0".to_string())));
    }

    /// A build without `devtools` does not so much as look the variable up.
    #[cfg(not(feature = "devtools"))]
    #[test]
    fn a_release_build_never_reads_the_software_override() {
        assert!(!software_forced_in_this_build(|name| panic!("read {name}")));
    }

    #[test]
    fn forcing_software_takes_the_software_mft_over_hardware_as_a_fallback() {
        let adapters = [gpu("RTX", VENDOR_NVIDIA), basic_render()];
        let encoders = [hw("NVIDIA H.264 Encoder MFT", "VEN_10DE"), software()];
        assert_eq!(choose(&adapters, &encoders, false), rank(&adapters, &encoders));
        assert_eq!(choose(&adapters, &encoders, false), Choice::Hardware { encoder: 0, adapter: 0 });

        let forced = choose(&adapters, &encoders, true);
        assert_eq!(
            forced,
            Choice::SoftwareFallback {
                encoder: 1,
                reason: "forced by NINJA_OWN_FORCE_SOFTWARE_ENCODER (devtools)".to_string()
            }
        );
        assert!(forced.is_fallback());
    }

    #[test]
    fn forcing_software_with_no_software_mft_is_unavailable_not_hardware() {
        let choice = choose(&[gpu("RTX", VENDOR_NVIDIA)], &[hw("NVIDIA H.264 Encoder MFT", "VEN_10DE")], true);
        let Choice::Unavailable { reason } = &choice else { panic!("{choice:?}") };
        assert!(reason.contains(FORCED_SOFTWARE_REASON) && reason.contains("no software"), "{reason}");
    }

    #[test]
    fn every_preset_is_recorded_since_238() {
        for preset in [
            AudioPreset::Game,
            AudioPreset::GameMic { mic_device_id: None },
            AudioPreset::GameMicDiscord { mic_device_id: Some("{0.0.1.00000000}.{abc}".into()) },
            AudioPreset::Desktop,
            AudioPreset::Unknown,
        ] {
            assert_eq!(audio_layout(&preset).unwrap(), preset.layout(), "{preset:?}");
        }
        let desktop_only = AudioPreset::Custom {
            sources: vec![AudioSourceKind::Desktop],
            tracks: vec![AudioTrackSpec { label: "Desktop".into(), sources: vec![0] }],
        };
        assert_eq!(audio_layout(&desktop_only).unwrap().tracks[0].label, "Desktop");
    }

    /// A hand-edited `Custom` row that points at a source it does not define
    /// is refused before anything opens, with the reason.
    #[test]
    fn a_custom_layout_that_is_not_one_is_refused() {
        let broken = AudioPreset::Custom {
            sources: vec![AudioSourceKind::Game],
            tracks: vec![AudioTrackSpec { label: "Nope".into(), sources: vec![3] }],
        };
        let reason = audio_layout(&broken).unwrap_err();
        assert!(reason.contains("cannot be recorded") && reason.contains("source 3"), "{reason}");
        let empty = AudioPreset::Custom { sources: vec![], tracks: vec![] };
        assert!(audio_layout(&empty).is_err());
    }
}
