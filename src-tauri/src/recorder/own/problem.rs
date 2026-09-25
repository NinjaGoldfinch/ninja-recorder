//! Which of the own backend's capture outcomes are failures worth telling
//! someone about, and which are the preset meeting a machine that does not
//! have what it names (#10). Pure, so every case is a unit test; `own/win/`
//! reports what happened and asks.
//!
//! An audio source can end up out of a recording at two stages:
//!
//! - **Find**: working out what to capture. The process tree (`root`), or the
//!   endpoint device (`GetDefaultAudioEndpoint`, `GetDevice`). For the
//!   microphone, the desktop and an application this failing *is* the
//!   absence: no microphone plugged in, no output device, Discord not
//!   running. The game is the exception, because a recording only starts once
//!   its window is there, so a game whose process tree cannot be found is a
//!   failure.
//! - **Open**: capturing what was found. Process-loopback activation,
//!   `IAudioClient::Initialize`, `Start`, the audio thread itself. Something
//!   that exists and cannot be captured is always a failure: it is the case
//!   the Windows 10 floor (`select::MIN_BUILD`) rests on, and the case a
//!   denied microphone permission produces.
//!
//! A source that opened and then stopped before the recording did is always a
//! failure too ([`ended`]), and so is a recording the worker could not see
//! through to its stop ([`stop_problem`]).

use super::plan::source_name;
use crate::recorder::audio::AudioSourceKind;
use crate::recorder::problem::CaptureProblem;

/// Where a source's capture stopped short. See the module comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Find,
    Open,
}

/// Why a source did not open, and at which stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError {
    pub stage: Stage,
    /// What the failing call said, with its HRESULT where there is one.
    pub reason: String,
}

impl SourceError {
    pub fn find(reason: impl Into<String>) -> SourceError {
        SourceError { stage: Stage::Find, reason: reason.into() }
    }

    pub fn open(reason: impl Into<String>) -> SourceError {
        SourceError { stage: Stage::Open, reason: reason.into() }
    }
}

/// A failing call's message is an open failure unless the caller says it was
/// a find: that is the conservative default, and it lets `?` carry every
/// `String` error the capture code already returns.
impl From<String> for SourceError {
    fn from(reason: String) -> SourceError {
        SourceError::open(reason)
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

/// Whether a source of `kind` that did not open at `stage` is a failure, as
/// against something the machine simply does not have.
pub fn is_failure(kind: &AudioSourceKind, stage: Stage) -> bool {
    match stage {
        Stage::Open => true,
        Stage::Find => matches!(kind, AudioSourceKind::Game),
    }
}

/// The problem a source that did not open amounts to, or `None` for an
/// absence.
pub fn not_opened(kind: &AudioSourceKind, error: &SourceError) -> Option<CaptureProblem> {
    is_failure(kind, error.stage).then(|| CaptureProblem::SourceFailed {
        source: source_name(kind),
        reason: error.reason.clone(),
    })
}

/// A source named `name` (as `plan::source_name` names it) that stopped
/// before the recording did.
pub fn ended(name: &str, reason: &str) -> CaptureProblem {
    CaptureProblem::SourceEnded { source: name.to_string(), reason: reason.to_string() }
}

/// What the daemon heard back from a stop, reduced to what decides whether the
/// recording ended early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopAnswer<'a> {
    /// Stopped when asked, and finalized.
    Clean,
    /// The worker's recording ended on its own before the stop (the GPU
    /// device lost, a write failing) and was finalized then.
    EndedEarly(&'a str),
    /// The worker's finalize failed; the fragments on disk are kept.
    FinalizeFailed(&'a str),
    /// No answer: the worker died, with why if the daemon saw it go.
    WorkerGone(Option<&'a str>),
}

/// The problem a stop whose file is being kept amounts to, or `None` for a
/// clean one. A stop that kept nothing is `NotSaved`, which the supervisor
/// says from the error `stop` returns.
pub fn stop_problem(answer: StopAnswer) -> Option<CaptureProblem> {
    let reason = match answer {
        StopAnswer::Clean => return None,
        StopAnswer::EndedEarly(why) => why.to_string(),
        StopAnswer::FinalizeFailed(why) => {
            format!("the file could not be finished ({why}); it plays up to its last fragment")
        }
        StopAnswer::WorkerGone(Some(why)) => format!("the capture worker stopped: {why}"),
        StopAnswer::WorkerGone(None) => "the capture worker stopped mid-recording".to_string(),
    };
    Some(CaptureProblem::EndedEarly { reason })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<AudioSourceKind> {
        vec![
            AudioSourceKind::Game,
            AudioSourceKind::Microphone { device_id: None },
            AudioSourceKind::Microphone { device_id: Some("{0.0.1.00000000}.{abc}".into()) },
            AudioSourceKind::Desktop,
            AudioSourceKind::Application { exe: "Discord.exe".into() },
        ]
    }

    /// Not there is not a failure, except for the game, whose window being
    /// there is what started the recording.
    #[test]
    fn a_source_that_is_not_there_is_an_absence_except_the_game() {
        for kind in every_kind() {
            let expected = matches!(kind, AudioSourceKind::Game);
            assert_eq!(is_failure(&kind, Stage::Find), expected, "{kind:?}");
        }
    }

    /// There and not capturable is always a failure: the Windows 10 case.
    #[test]
    fn a_source_that_is_there_and_will_not_open_is_a_failure() {
        for kind in every_kind() {
            assert!(is_failure(&kind, Stage::Open), "{kind:?}");
        }
    }

    #[test]
    fn discord_not_running_is_said_nowhere_but_the_log() {
        let discord = AudioSourceKind::Application { exe: "Discord.exe".into() };
        assert_eq!(not_opened(&discord, &SourceError::find("no Discord.exe process is running")), None);
        let mic = AudioSourceKind::Microphone { device_id: None };
        assert_eq!(
            not_opened(&mic, &SourceError::find("there is no default microphone: Element not found. (0x80070490)")),
            None
        );
        assert_eq!(not_opened(&AudioSourceKind::Desktop, &SourceError::find("no output")), None);
    }

    #[test]
    fn a_refused_process_loopback_is_a_problem_carrying_the_call() {
        let reason = "process-loopback activation for PID 4242 was refused: Access is denied. (0x80070005)";
        assert_eq!(
            not_opened(&AudioSourceKind::Game, &SourceError::open(reason)),
            Some(CaptureProblem::SourceFailed { source: "game".into(), reason: reason.into() })
        );
        let initialize = "IAudioClient::Initialize (the microphone) failed: Access is denied. (0x80070005)";
        let mic = AudioSourceKind::Microphone { device_id: None };
        assert_eq!(
            not_opened(&mic, &SourceError::open(initialize)),
            Some(CaptureProblem::SourceFailed { source: "microphone".into(), reason: initialize.into() })
        );
        let discord = AudioSourceKind::Application { exe: "Discord.exe".into() };
        let Some(CaptureProblem::SourceFailed { source, .. }) =
            not_opened(&discord, &SourceError::open("x"))
        else {
            panic!("an application that is running and cannot be captured is a failure");
        };
        assert_eq!(source, "Discord.exe");
    }

    #[test]
    fn the_game_tree_missing_is_a_problem() {
        let why = "no League of Legends.exe process is running (no game window owner)";
        assert!(not_opened(&AudioSourceKind::Game, &SourceError::find(why)).is_some());
    }

    #[test]
    fn a_string_error_is_an_open_failure() {
        let error: SourceError = "IAudioClient::Start failed".to_string().into();
        assert_eq!(error.stage, Stage::Open);
        assert_eq!(error.to_string(), "IAudioClient::Start failed");
    }

    #[test]
    fn a_source_ending_early_is_always_a_problem() {
        assert_eq!(
            ended("microphone", "GetNextPacketSize failed: (0x88890004)"),
            CaptureProblem::SourceEnded {
                source: "microphone".into(),
                reason: "GetNextPacketSize failed: (0x88890004)".into()
            }
        );
    }

    #[test]
    fn a_stop_that_kept_a_file_early_says_why() {
        assert_eq!(stop_problem(StopAnswer::Clean), None);
        assert_eq!(
            stop_problem(StopAnswer::EndedEarly("the GPU device was lost (0x887A0005)")),
            Some(CaptureProblem::EndedEarly { reason: "the GPU device was lost (0x887A0005)".into() })
        );
        let Some(CaptureProblem::EndedEarly { reason }) =
            stop_problem(StopAnswer::FinalizeFailed("MF_E_X"))
        else {
            panic!()
        };
        assert!(reason.contains("MF_E_X") && reason.contains("last fragment"), "{reason}");
        let Some(CaptureProblem::EndedEarly { reason }) =
            stop_problem(StopAnswer::WorkerGone(Some("capture worker pid 7 exited with code -1073741819")))
        else {
            panic!()
        };
        assert!(reason.contains("-1073741819"), "{reason}");
        assert!(stop_problem(StopAnswer::WorkerGone(None)).is_some());
    }
}
