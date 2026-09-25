//! What a recording is missing because something that should have worked did
//! not, and the words a person is told about it (#10).
//!
//! **A problem is a failure, never an absence.** A preset that names Discord
//! on a machine where Discord is not running, or a microphone on one with none
//! plugged in, records without it, and that is the preset working as designed:
//! nothing is said beyond the log. A source that is there and fails to open
//! (process loopback refused, `IAudioClient::Initialize` failing), a source
//! that stops part-way through the game, a recording that ends before the
//! game does, and a start the backend refuses outright are what this module
//! describes. Which is which, for the own backend's audio sources, is decided
//! in `own::problem`; everything else here is backend-agnostic.
//!
//! A `CaptureProblem` crosses three boundaries, which is why it is plain data:
//! the capture worker's pipe (`own::worker::protocol`, as optional fields), the
//! `Recorder` trait (`RecordingOutput::problems`), and the contract, as the
//! `captureProblems` event and in `recordings.diagnostics_json`. The daemon
//! turns it into a desktop notification (`notification`, below); the UI turns
//! it into a strip and a line on the recording (`src/lib/library/problems.ts`).
//!
//! **The reason is technical text and untrusted input.** It is whatever the
//! failing call said, with its HRESULT where Windows gave one, because that is
//! what makes a bug report useful. It is only ever interpolated as text.

use serde::{Deserialize, Serialize};

/// One thing a recording lost to a failure.
///
/// `source` names an audio source the way the log does: `game`,
/// `microphone`, `desktop`, or an application's executable (`Discord.exe`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureProblem {
    /// A source the preset names, which was there to capture, did not open.
    /// The file has no such audio at all, and the rest of it is as normal.
    SourceFailed { source: String, reason: String },
    /// A source that opened stopped before the recording did, and is silence
    /// from then on in every track it feeds.
    SourceEnded { source: String, reason: String },
    /// The recording stopped before the game did (the GPU device was lost, a
    /// write failed, the capture worker died). What was written is kept.
    EndedEarly { reason: String },
    /// Nothing was recorded: the backend refused to start.
    NotStarted { reason: String },
    /// The recording could not be finished, and nothing was kept.
    NotSaved { reason: String },
}

impl CaptureProblem {
    /// What the failing call said.
    pub fn reason(&self) -> &str {
        match self {
            CaptureProblem::SourceFailed { reason, .. }
            | CaptureProblem::SourceEnded { reason, .. }
            | CaptureProblem::EndedEarly { reason }
            | CaptureProblem::NotStarted { reason }
            | CaptureProblem::NotSaved { reason } => reason,
        }
    }

    /// Whether the recording itself is gone, as against missing a part.
    pub fn is_whole(&self) -> bool {
        matches!(self, CaptureProblem::NotStarted { .. } | CaptureProblem::NotSaved { .. })
    }

    /// One sentence for a person, without the reason: `No game audio`, `The
    /// microphone stopped part-way`, `The recording ended early`.
    pub fn headline(&self) -> String {
        match self {
            CaptureProblem::SourceFailed { source, .. } => format!("No {}", source_label(source)),
            CaptureProblem::SourceEnded { source, .. } => {
                format!("{} stopped part-way", capitalise(&source_label(source)))
            }
            CaptureProblem::EndedEarly { .. } => "The recording ended early".to_string(),
            CaptureProblem::NotStarted { .. } => "The game was not recorded".to_string(),
            CaptureProblem::NotSaved { .. } => "The recording could not be saved".to_string(),
        }
    }
}

/// How a source is named to a person: `game audio`, `microphone audio`,
/// `desktop audio`, or `Discord audio` for `Discord.exe`.
pub fn source_label(source: &str) -> String {
    match source {
        "game" | "microphone" | "desktop" => format!("{source} audio"),
        exe => {
            let stem = exe.len().checked_sub(4).filter(|&cut| {
                exe.is_char_boundary(cut) && exe[cut..].eq_ignore_ascii_case(".exe")
            });
            format!("{} audio", stem.map_or(exe, |cut| &exe[..cut]))
        }
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `build 19045`, or `an unknown build` when it could not be read, as the
/// notification and the log name the Windows it happened on.
pub fn build_phrase(windows_build: Option<u32>) -> String {
    match windows_build {
        Some(build) => format!("Windows build {build}"),
        None => "an unknown Windows build".to_string(),
    }
}

/// The Windows build this is running on, or `None` off Windows or when it
/// cannot be read. For the notification and the diagnostics: a capture
/// failure without the build it happened on is half a bug report.
pub fn windows_build() -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        crate::recorder::own::windows_build()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// The desktop notification for a recording that was saved with problems:
/// its title and body, or `None` when there were none. **One per recording**,
/// whatever it lost, so the title names everything missing and the body gives
/// each reason once.
///
/// `name` is the file's stem, as the "Recording saved" toast names it.
pub fn notification(
    name: &str,
    problems: &[CaptureProblem],
    windows_build: Option<u32>,
) -> Option<(String, String)> {
    if problems.is_empty() {
        return None;
    }
    let missing: Vec<String> = problems
        .iter()
        .filter_map(|p| match p {
            CaptureProblem::SourceFailed { source, .. } | CaptureProblem::SourceEnded { source, .. } => {
                Some(source_label(source))
            }
            _ => None,
        })
        .collect();
    let ended = problems.iter().any(|p| matches!(p, CaptureProblem::EndedEarly { .. }));
    let title = match (missing.is_empty(), ended) {
        (true, _) => "Recording ended early".to_string(),
        (false, false) => format!("Recording saved without {}", join_and(&missing)),
        (false, true) => format!("Recording ended early, without {}", join_and(&missing)),
    };
    let reasons: Vec<String> =
        problems.iter().map(|p| format!("{}: {}.", p.headline(), trim_stop(p.reason()))).collect();
    let body = format!(
        "{name}. {} Please report this with your Windows version ({}).",
        reasons.join(" "),
        build_phrase(windows_build)
    );
    Some((title, body))
}

/// The message for a start the backend refused, for the existing "Recording
/// problem" notification: the error and the build it happened on.
pub fn refused_message(error: &str, windows_build: Option<u32>) -> String {
    format!("The recording could not be started: {} ({}).", trim_stop(error), build_phrase(windows_build))
}

/// `a`, `a and b`, `a, b and c`.
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// A reason without the full stop it may already end in, so a sentence built
/// around it gets exactly one.
fn trim_stop(reason: &str) -> &str {
    reason.trim().trim_end_matches('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(source: &str, reason: &str) -> CaptureProblem {
        CaptureProblem::SourceFailed { source: source.into(), reason: reason.into() }
    }

    #[test]
    fn sources_are_named_for_a_person() {
        assert_eq!(source_label("game"), "game audio");
        assert_eq!(source_label("microphone"), "microphone audio");
        assert_eq!(source_label("desktop"), "desktop audio");
        assert_eq!(source_label("Discord.exe"), "Discord audio");
        assert_eq!(source_label("discord.EXE"), "discord audio");
        assert_eq!(source_label("tool"), "tool audio");
        assert_eq!(source_label(".exe"), " audio");
        // Not a byte-slicing panic on a name that ends in a multi-byte char.
        assert_eq!(source_label("naïve"), "naïve audio");
    }

    #[test]
    fn each_problem_has_a_headline() {
        assert_eq!(failed("game", "x").headline(), "No game audio");
        let ended = CaptureProblem::SourceEnded { source: "microphone".into(), reason: "x".into() };
        assert_eq!(ended.headline(), "Microphone audio stopped part-way");
        assert_eq!(CaptureProblem::EndedEarly { reason: "x".into() }.headline(), "The recording ended early");
        assert!(CaptureProblem::NotStarted { reason: "x".into() }.is_whole());
        assert!(CaptureProblem::NotSaved { reason: "x".into() }.is_whole());
        assert!(!failed("game", "x").is_whole());
    }

    /// The owner's example, made a test: the source, the call that failed
    /// with its HRESULT, and the build, in one notification.
    #[test]
    fn the_notification_names_the_source_the_call_and_the_build() {
        let reason = "process-loopback activation for PID 4242 was refused: Access is denied. \
                      (0x80070005)";
        let (title, body) =
            notification("recording-1", &[failed("game", reason)], Some(19_045)).unwrap();
        assert_eq!(title, "Recording saved without game audio");
        assert_eq!(
            body,
            "recording-1. No game audio: process-loopback activation for PID 4242 was refused: \
             Access is denied. (0x80070005). Please report this with your Windows version \
             (Windows build 19045)."
        );
    }

    #[test]
    fn one_notification_covers_every_problem() {
        let problems = [
            failed("game", "refused (0x80070005)"),
            CaptureProblem::SourceEnded { source: "Discord.exe".into(), reason: "gone.".into() },
        ];
        let (title, body) = notification("r", &problems, None).unwrap();
        assert_eq!(title, "Recording saved without game audio and Discord audio");
        assert!(body.contains("No game audio: refused (0x80070005)."), "{body}");
        assert!(body.contains("Discord audio stopped part-way: gone."), "{body}");
        assert!(!body.contains(".."), "one full stop per sentence: {body}");
        assert!(body.ends_with("(an unknown Windows build)."), "{body}");
    }

    #[test]
    fn an_early_end_is_the_title_when_it_happens() {
        let early = CaptureProblem::EndedEarly { reason: "the GPU device was lost".into() };
        let (title, _) = notification("r", std::slice::from_ref(&early), Some(1)).unwrap();
        assert_eq!(title, "Recording ended early");
        let (title, _) = notification("r", &[early, failed("microphone", "x")], Some(1)).unwrap();
        assert_eq!(title, "Recording ended early, without microphone audio");
    }

    #[test]
    fn nothing_to_say_is_no_notification() {
        assert_eq!(notification("r", &[], Some(19_045)), None);
    }

    #[test]
    fn a_refused_start_names_the_build() {
        assert_eq!(
            refused_message("recorder backend error: no frame from WGC.", Some(19_041)),
            "The recording could not be started: recorder backend error: no frame from WGC \
             (Windows build 19041)."
        );
        assert!(refused_message("x", None).ends_with("(an unknown Windows build)."));
    }

    #[test]
    fn joins_read_as_a_list() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(join_and(&s(&["a"])), "a");
        assert_eq!(join_and(&s(&["a", "b"])), "a and b");
        assert_eq!(join_and(&s(&["a", "b", "c"])), "a, b and c");
    }

    /// The wire shape the worker, the event and the stored diagnostics share.
    #[test]
    fn the_wire_shape_is_tagged_camel_case() {
        let json = serde_json::to_value(failed("game", "r")).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "sourceFailed", "source": "game", "reason": "r"}));
        let json = serde_json::to_value(CaptureProblem::EndedEarly { reason: "r".into() }).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "endedEarly", "reason": "r"}));
        for problem in [
            failed("Discord.exe", "a\nb"),
            CaptureProblem::SourceEnded { source: "microphone".into(), reason: "c".into() },
            CaptureProblem::EndedEarly { reason: "d".into() },
            CaptureProblem::NotStarted { reason: "e".into() },
            CaptureProblem::NotSaved { reason: "f".into() },
        ] {
            let back: CaptureProblem =
                serde_json::from_str(&serde_json::to_string(&problem).unwrap()).unwrap();
            assert_eq!(back, problem);
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn off_windows_there_is_no_build() {
        assert_eq!(windows_build(), None);
    }
}
