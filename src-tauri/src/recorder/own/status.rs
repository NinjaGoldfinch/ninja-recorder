//! What a capture session decides that needs no Windows to decide: the frame
//! size it encodes at, whether the encoder Media Foundation loaded is the one
//! [`select::rank`] chose, and how the backend describes itself.
//!
//! `own/win/` asks these questions with real COM objects in hand and passes
//! plain values here, so every answer is a unit test on any host.

use super::select::{self, Adapter, Choice, Encoder};

/// The size a `width` x `height` capture is encoded at: each dimension
/// rounded **down** to even, because H.264's 4:2:0 chroma is subsampled by
/// two in both directions and a window can be any size. `None` when either
/// is zero afterwards, which is a minimised window or one with no client
/// area yet: nothing to encode.
///
/// Down rather than up, so the encoded frame never asks for a column or row
/// the capture does not have. The one it drops is at the right or bottom edge.
pub fn even_size(width: i32, height: i32) -> Option<(u32, u32)> {
    let even = |v: i32| (v.max(0) as u32) & !1;
    let (w, h) = (even(width), even(height));
    (w > 0 && h > 0).then_some((w, h))
}

/// What the sink writer actually loaded for the video stream, read back from
/// the encoder transform's own attributes after `BeginWriting`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    /// `MFT_FRIENDLY_NAME_Attribute`.
    pub name: Option<String>,
    /// `MFT_ENUM_HARDWARE_VENDOR_ID_Attribute`, `VEN_xxxx`.
    pub vendor: Option<String>,
    /// `MFT_ENUM_HARDWARE_URL_Attribute`.
    pub url: Option<String>,
}

impl Loaded {
    /// A hardware MFT carries a vendor id or a hardware URL; the software
    /// one carries neither. The same test the spike (`p0c-video`) used.
    pub fn hardware(&self) -> bool {
        self.vendor.is_some() || self.url.is_some()
    }

    /// The loaded encoder's name, with its vendor id when it has one.
    ///
    /// Microsoft's software H.264 MFT carries no friendly name once loaded
    /// (the CI test on `windows-latest` shows it), so a nameless encoder with
    /// no hardware attributes is named for what it is.
    fn describe(&self) -> String {
        let name = match (&self.name, self.hardware()) {
            (Some(name), _) => name.as_str(),
            (None, false) => "the software H.264 MFT",
            (None, true) => "a hardware encoder that reports no name",
        };
        match &self.vendor {
            Some(vendor) => format!("{name} [{vendor}]"),
            None => name.to_string(),
        }
    }
}

/// Where the own backend stands, which is what `backend_name` says and so
/// what `RecordingDiagnostics::backend` records for every file.
///
/// Serialized because the capture worker reports it over its pipe
/// (`worker::protocol`). That pipe is not the UI contract, so no `TS`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Status {
    /// Nothing brought up yet, or released.
    Idle,
    /// Hardware encoding, on the named encoder.
    Ready { encoder: String },
    /// The software MFT, and why. Allowed, never silent (DEVELOPMENT.md §2.4).
    Software { encoder: String, reason: String },
    /// Cannot record, and why.
    Unavailable { reason: String },
}

impl Status {
    /// What [`select::rank`] chose, before anything is loaded: the pre-warm's
    /// answer. `encoders` is the list `rank` indexed into.
    pub fn from_choice(choice: &Choice, encoders: &[Encoder]) -> Status {
        let name = |i: usize| {
            encoders.get(i).map_or_else(|| "an unnamed encoder".to_string(), |e| e.name.clone())
        };
        match choice {
            Choice::Hardware { encoder, .. } => Status::Ready { encoder: name(*encoder) },
            Choice::SoftwareFallback { encoder, reason } => {
                Status::Software { encoder: name(*encoder), reason: reason.clone() }
            }
            Choice::Unavailable { reason } => Status::Unavailable { reason: reason.clone() },
        }
    }

    /// The line `Recorder::backend_name` returns.
    pub fn backend_name(&self) -> String {
        match self {
            Status::Idle => "own (idle)".to_string(),
            Status::Ready { encoder } => format!("own (ready: {encoder})"),
            Status::Software { encoder, reason } => {
                format!("own (software encoding: {encoder}, because {reason})")
            }
            Status::Unavailable { reason } => format!("own (unavailable: {reason})"),
        }
    }
}

/// Checks what Media Foundation loaded against what [`select::rank`] chose.
///
/// The sink writer picks its own encoder from the media types and the device
/// it was given, so a ranked choice is a request, not a guarantee. What it
/// loaded is what the file is made with, and that is what the status has to
/// name:
///
/// - hardware ranked, a hardware encoder of that adapter's vendor loaded:
///   ready;
/// - hardware ranked, the software MFT loaded: a software fallback, with the
///   substitution as its reason, because a silent one is exactly what §2.4
///   rules out;
/// - hardware ranked, another vendor's hardware encoder loaded: ready, with
///   a warning for the log, because it is hardware and still records;
/// - software ranked: the fallback with `rank`'s own reason (or ready, should
///   the sink writer have found hardware after all).
///
/// Returns the status and, separately, a warning the caller logs. `Err` for
/// [`Choice::Unavailable`], which never reaches a sink writer.
pub fn check_loaded(
    choice: &Choice,
    adapters: &[Adapter],
    encoders: &[Encoder],
    loaded: &Loaded,
) -> Result<(Status, Option<String>), String> {
    let name = loaded.describe();
    match choice {
        Choice::Unavailable { reason } => Err(reason.clone()),
        Choice::Hardware { encoder, adapter } => {
            let ranked = encoders.get(*encoder).map_or("the ranked encoder", |e| e.name.as_str());
            if !loaded.hardware() {
                return Ok((
                    Status::Software {
                        encoder: name.clone(),
                        reason: format!(
                            "Media Foundation loaded {name} instead of the hardware encoder \
                             {ranked}"
                        ),
                    },
                    None,
                ));
            }
            let expected = adapters.get(*adapter).map(|a| a.vendor);
            let got = loaded.vendor.as_deref().and_then(select::vendor_id);
            let warning = (got != expected).then(|| {
                format!(
                    "Media Foundation loaded {name}, not {ranked} on {}: still hardware \
                     encoding, but not on the adapter that was ranked",
                    expected.map_or("an unknown vendor", select::vendor_name)
                )
            });
            Ok((Status::Ready { encoder: name }, warning))
        }
        Choice::SoftwareFallback { reason, .. } => {
            if loaded.hardware() {
                Ok((Status::Ready { encoder: name }, None))
            } else {
                Ok((Status::Software { encoder: name, reason: reason.clone() }, None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::own::select::VENDOR_NVIDIA;

    #[test]
    fn dimensions_round_down_to_even() {
        assert_eq!(even_size(1920, 1080), Some((1920, 1080)));
        assert_eq!(even_size(1919, 1081), Some((1918, 1080)));
        assert_eq!(even_size(3, 3), Some((2, 2)));
    }

    #[test]
    fn nothing_to_encode_is_none() {
        assert_eq!(even_size(0, 1080), None);
        assert_eq!(even_size(1920, 0), None);
        // A 1x1 client rect is a window not ready yet, and rounds to zero.
        assert_eq!(even_size(1, 1), None);
        assert_eq!(even_size(-5, 720), None);
    }

    fn nvidia() -> (Vec<Adapter>, Vec<Encoder>) {
        (
            vec![Adapter { name: "RTX".into(), vendor: VENDOR_NVIDIA, software: false }],
            vec![
                Encoder {
                    name: "NVIDIA H.264 Encoder MFT".into(),
                    vendor: Some("VEN_10DE".into()),
                    hardware: true,
                },
                Encoder { name: "H264 Encoder MFT".into(), vendor: None, hardware: false },
            ],
        )
    }

    fn loaded(name: &str, vendor: Option<&str>) -> Loaded {
        Loaded { name: Some(name.into()), vendor: vendor.map(Into::into), url: None }
    }

    #[test]
    fn the_ranked_hardware_encoder_loaded_is_ready() {
        let (adapters, encoders) = nvidia();
        let choice = select::rank(&adapters, &encoders);
        let (status, warning) = check_loaded(
            &choice,
            &adapters,
            &encoders,
            &loaded("NVIDIA H.264 Encoder MFT", Some("VEN_10DE")),
        )
        .unwrap();
        assert_eq!(status.backend_name(), "own (ready: NVIDIA H.264 Encoder MFT [VEN_10DE])");
        assert_eq!(warning, None);
    }

    /// The case §2.4 is about: the sink writer quietly swapped in software.
    #[test]
    fn software_loaded_in_place_of_hardware_is_surfaced_as_a_fallback() {
        let (adapters, encoders) = nvidia();
        let choice = select::rank(&adapters, &encoders);
        let (status, _) =
            check_loaded(&choice, &adapters, &encoders, &loaded("H264 Encoder MFT", None)).unwrap();
        let Status::Software { encoder, reason } = &status else { panic!("{status:?}") };
        assert_eq!(encoder, "H264 Encoder MFT");
        assert!(reason.contains("instead of the hardware encoder NVIDIA H.264"), "{reason}");
        assert!(status.backend_name().starts_with("own (software encoding: H264 Encoder MFT"));
    }

    #[test]
    fn another_vendors_hardware_is_ready_with_a_warning() {
        let (adapters, encoders) = nvidia();
        let choice = select::rank(&adapters, &encoders);
        let (status, warning) = check_loaded(
            &choice,
            &adapters,
            &encoders,
            &loaded("Intel Quick Sync H.264 Encoder MFT", Some("VEN_8086")),
        )
        .unwrap();
        assert!(matches!(status, Status::Ready { .. }), "{status:?}");
        let warning = warning.expect("a warning");
        assert!(warning.contains("NVIDIA"), "{warning}");
    }

    /// What `windows-latest` loads: the software MFT, with no name at all.
    #[test]
    fn a_nameless_software_encoder_is_named_for_what_it_is() {
        let (adapters, encoders) = nvidia();
        let choice = select::rank(&adapters, &encoders);
        let (status, _) =
            check_loaded(&choice, &adapters, &encoders, &Loaded::default()).unwrap();
        assert!(
            status.backend_name().starts_with("own (software encoding: the software H.264 MFT"),
            "{status:?}"
        );
    }

    #[test]
    fn a_hardware_url_alone_counts_as_hardware() {
        let found = Loaded { name: None, vendor: None, url: Some("AMDh264Encoder".into()) };
        assert!(found.hardware());
        assert!(!Loaded::default().hardware());
    }

    #[test]
    fn the_ranked_fallback_keeps_its_reason() {
        let adapters = vec![Adapter { name: "Basic".into(), vendor: 0x1414, software: true }];
        let encoders =
            vec![Encoder { name: "H264 Encoder MFT".into(), vendor: None, hardware: false }];
        let choice = select::rank(&adapters, &encoders);
        assert_eq!(
            Status::from_choice(&choice, &encoders),
            Status::Software {
                encoder: "H264 Encoder MFT".into(),
                reason: "no hardware GPU was found".into()
            }
        );
        let (status, warning) =
            check_loaded(&choice, &adapters, &encoders, &loaded("H264 Encoder MFT", None)).unwrap();
        assert_eq!(
            status.backend_name(),
            "own (software encoding: H264 Encoder MFT, because no hardware GPU was found)"
        );
        assert_eq!(warning, None);
    }

    #[test]
    fn unavailable_refuses_with_its_reason() {
        let choice = Choice::Unavailable { reason: "nothing".into() };
        assert_eq!(check_loaded(&choice, &[], &[], &Loaded::default()), Err("nothing".into()));
        assert_eq!(
            Status::from_choice(&choice, &[]).backend_name(),
            "own (unavailable: nothing)"
        );
    }

    #[test]
    fn the_names_the_diagnostics_record() {
        assert_eq!(Status::Idle.backend_name(), "own (idle)");
        let (_, encoders) = nvidia();
        let choice = Choice::Hardware { encoder: 0, adapter: 0 };
        assert_eq!(
            Status::from_choice(&choice, &encoders).backend_name(),
            "own (ready: NVIDIA H.264 Encoder MFT)"
        );
    }
}
