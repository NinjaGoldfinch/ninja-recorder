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
//! says why, so no caller can use it without knowing.
//!
//! It also answers the two other "can it record this" questions: the Windows
//! build floor ([`availability`]), and which audio presets the backend can
//! record yet ([`audio_layout`]).

use crate::recorder::audio::{AudioLayout, AudioPreset, AudioSourceKind};

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

/// The oldest Windows build the own backend runs on.
///
/// The stricter of the two APIs it cannot do without:
///
/// - **Process loopback** (`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`),
///   for game audio on its own track: "Minimum supported client: Windows 10
///   Build 20348", per Microsoft Learn's `AUDIOCLIENT_ACTIVATION_TYPE` page,
///   and the ApplicationLoopback sample's README says the same.
/// - **WGC window capture** (`IGraphicsCaptureItemInterop::CreateForWindow`):
///   Windows 10 1903, build 18362, per its Microsoft Learn page.
///
/// OBS registers its process-output source from 19041 instead ("MS says
/// 20348, but process filtering seems to work earlier", `win-wasapi`'s
/// `plugin-main.cpp`), which would take in Windows 10 2004 through 22H2
/// (19041–19045). That is unverified here, so the floor is the documented
/// one; lowering it is a measurement on a Windows 10 box, for #237, and the
/// test pinning this constant is what has to change with it.
pub const MIN_BUILD: u32 = 20_348;

/// `None` if the own backend can run on Windows build `build`, or the reason
/// it cannot, for the backend chooser to show.
pub fn availability(build: u32) -> Option<String> {
    (build < MIN_BUILD).then(|| {
        format!(
            "the own capture backend needs Windows build {MIN_BUILD} or newer (Windows 11), \
             for per-application audio capture; this is build {build}"
        )
    })
}

/// The environment variable that lets a **devtools** build construct the own
/// backend below [`MIN_BUILD`], so it can be tried on Windows 10 before the
/// floor moves (#237). A release build never reads it.
pub const IGNORE_FLOOR_ENV: &str = "NINJA_OWN_IGNORE_OS_FLOOR";

/// Whether the floor override is in force: only in a devtools build, and
/// only for the value `1`. Pure, so the release half is a test; the daemon
/// passes `cfg!(feature = "devtools")` and the variable's value.
pub fn floor_ignored(devtools: bool, value: Option<&str>) -> bool {
    devtools && value == Some("1")
}

/// The audio layout the own backend can record for `preset`, or why not.
///
/// **Only the Game preset until #238 and #239.** The sink writer this piece
/// records through holds one audio stream, and the only source captured so
/// far is the game's, by process loopback. Every other preset names a second
/// source (a microphone, Discord, the desktop) and two to four tracks, so it
/// is refused with the reason rather than recorded as game audio alone: a
/// file that quietly left out a source the preset names is the bug §2.5
/// warns about. A `Custom` layout that is exactly one game track is the Game
/// preset by another name, and is accepted.
pub fn audio_layout(preset: &AudioPreset) -> Result<AudioLayout, String> {
    let layout = preset.layout();
    let game_only = layout.sources == [AudioSourceKind::Game]
        && layout.tracks.len() == 1
        && layout.tracks[0].sources == [0];
    if game_only {
        return Ok(layout);
    }
    Err(format!(
        "the own capture backend records the Game audio preset only, until the microphone, \
         desktop and application sources (#238) and multi-track files (#239) arrive; this \
         preset has {} source(s) on {} track(s). Choose the Game preset, or the libobs backend",
        layout.sources.len(),
        layout.tracks.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// (see `MIN_BUILD`), not a tidy-up.
    #[test]
    fn the_os_floor_is_build_20348() {
        assert_eq!(MIN_BUILD, 20_348);
        for (build, runs) in [
            (18_362, false), // 1903: WGC window capture, no process loopback
            (19_041, false), // 2004: where OBS enables process loopback
            (19_045, false), // Windows 10 22H2, the last Windows 10
            (20_347, false),
            (20_348, true),  // Server 2022, the documented floor
            (22_000, true),  // Windows 11 21H2
            (26_200, true),  // the verification box (DEVELOPMENT.md §16)
        ] {
            assert_eq!(availability(build).is_none(), runs, "build {build}");
        }
        let reason = availability(19_045).unwrap();
        assert!(reason.contains("20348") && reason.contains("19045"), "{reason}");
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
    fn only_the_game_preset_is_recorded_until_238() {
        assert_eq!(audio_layout(&AudioPreset::Game).unwrap(), AudioPreset::Game.layout());
        assert_eq!(audio_layout(&AudioPreset::Unknown).unwrap(), AudioPreset::Game.layout());
        for preset in [
            AudioPreset::GameMic { mic_device_id: None },
            AudioPreset::GameMicDiscord { mic_device_id: None },
            AudioPreset::Desktop,
        ] {
            let reason = audio_layout(&preset).unwrap_err();
            assert!(reason.contains("#238") && reason.contains("Game preset"), "{reason}");
        }
        // A custom layout that is one game track is the Game preset.
        let custom = AudioPreset::Custom {
            sources: vec![AudioSourceKind::Game],
            tracks: vec![crate::recorder::audio::AudioTrackSpec {
                label: "Just the game".into(),
                sources: vec![0],
            }],
        };
        assert_eq!(audio_layout(&custom).unwrap().tracks[0].label, "Just the game");
        let desktop_only = AudioPreset::Custom {
            sources: vec![AudioSourceKind::Desktop],
            tracks: vec![crate::recorder::audio::AudioTrackSpec {
                label: "Desktop".into(),
                sources: vec![0],
            }],
        };
        assert!(audio_layout(&desktop_only).is_err());
    }
}
