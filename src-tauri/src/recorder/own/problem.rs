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
//!
//! A failure is not always a bug. A microphone unplugged mid-game, or one
//! Windows' privacy settings refuse to desktop apps, is the user's machine
//! doing what it was told, and [`explain`] recognises those by their HRESULT
//! and says them in plain words, with a fix where there is one and no request
//! for a bug report (#296). The technical reason is kept either way.

use super::plan::source_name;
use crate::recorder::audio::AudioSourceKind;
use crate::recorder::problem::{CaptureProblem, Explained};

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
    is_failure(kind, error.stage).then(|| {
        let source = source_name(kind);
        let explained = explain(&source, &error.reason);
        CaptureProblem::SourceFailed { source, reason: error.reason.clone(), explained }
    })
}

/// A source named `name` (as `plan::source_name` names it) that stopped
/// before the recording did.
pub fn ended(name: &str, reason: &str) -> CaptureProblem {
    CaptureProblem::SourceEnded {
        source: name.to_string(),
        reason: reason.to_string(),
        explained: explain(name, reason),
    }
}

/// `AUDCLNT_E_DEVICE_INVALIDATED`: the endpoint went away. Unplugged,
/// switched off, disabled, or a wireless headset out of range.
const DEVICE_INVALIDATED: u32 = 0x8889_0004;

/// `E_ACCESSDENIED`. On the microphone this is Windows' privacy setting, or
/// a policy, refusing desktop apps the microphone; on a process loopback it
/// is the case the Windows 10 floor rests on, which stays a bug report.
const ACCESS_DENIED: u32 = 0x8007_0005;

/// What to do about [`ACCESS_DENIED`] on the microphone.
pub const MICROPHONE_ACCESS_FIX: &str = "Turn on \"Let desktop apps access your microphone\" in \
                                         Settings → Privacy & security → Microphone.";

/// What a source's failure means in words, when it is one of the few that
/// are the user's or the machine's doing rather than a bug (#296): no report
/// is asked for, and the call and HRESULT stay in the reason, for the log and
/// the stored diagnostics. `None` for everything else, which is told as the
/// reason and asks for a report.
///
/// `source` is named as `plan::source_name` names it.
pub fn explain(source: &str, reason: &str) -> Option<Explained> {
    let codes = hresults(reason);
    if codes.contains(&DEVICE_INVALIDATED) {
        let text = match source {
            "microphone" => "the microphone was disconnected",
            _ => "the output device was disconnected",
        };
        return Some(Explained { text: text.to_string(), fix: None, report: false });
    }
    if source == "microphone" && codes.contains(&ACCESS_DENIED) {
        return Some(Explained {
            text: "Windows is blocking microphone access".to_string(),
            fix: Some(MICROPHONE_ACCESS_FIX.to_string()),
            report: false,
        });
    }
    None
}

/// Every HRESULT a reason names, written as `0x` and eight hex digits, the
/// way a `windows::core::Error` and this module's own messages print them.
fn hresults(reason: &str) -> Vec<u32> {
    let lower = reason.to_ascii_lowercase();
    lower
        .split("0x")
        .skip(1)
        .filter_map(|rest| {
            let digits = rest.get(..8)?;
            let next_is_hex = rest[8..].chars().next().is_some_and(|c| c.is_ascii_hexdigit());
            if next_is_hex || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(digits, 16).ok()
        })
        .collect()
}

/// What the daemon heard back from a stop, reduced to what decides whether the
/// recording ended early.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopAnswer<'a> {
    /// Stopped when asked, and finalized.
    Clean,
    /// The worker's recording ended on its own before the stop (the GPU
    /// device lost, a write failing) and was finalized then.
    EndedEarly(&'a str),
    /// The worker's finalize failed; the fragments on disk are kept.
    FinalizeFailed(&'a str),
    /// No answer: the worker died, with why if the daemon saw it go, and
    /// how far into the recording the file it left reaches, if that could be
    /// read (#299).
    WorkerGone { why: Option<&'a str>, at_s: Option<f64> },
}

/// The problem a stop whose file is being kept amounts to, or `None` for a
/// clean one. A stop that kept nothing is `NotSaved`, which the supervisor
/// says from the error `stop` returns.
///
/// A dead worker is told in plain words, "the capture worker stopped
/// unexpectedly", with the pid and exit code kept in the reason for the log
/// and the stored diagnostics (#296). It still asks for a report: a worker
/// dying is not something the user did.
pub fn stop_problem(answer: StopAnswer) -> Option<CaptureProblem> {
    let (reason, explained) = match answer {
        StopAnswer::Clean => return None,
        StopAnswer::EndedEarly(why) => (why.to_string(), None),
        StopAnswer::FinalizeFailed(why) => (
            format!("the file could not be finished ({why}); it plays up to its last fragment"),
            None,
        ),
        StopAnswer::WorkerGone { why, at_s } => {
            let at = at_s.map_or_else(|| "mid-recording".to_string(), |s| format!("at {}", clock(s)));
            let reason = match why {
                Some(why) => format!("the capture worker stopped {at}: {why}"),
                None => format!("the capture worker stopped {at}"),
            };
            let told = Explained {
                text: format!("the capture worker stopped unexpectedly {at}"),
                fix: None,
                report: true,
            };
            (reason, Some(told))
        }
    };
    Some(CaptureProblem::EndedEarly { reason, explained })
}

/// `2:58`, or `1:02:03` past the hour: a point in the recording, as the
/// player's own clock shows it. Seconds are truncated, so the time named is
/// one the file reaches.
fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
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
            Some(CaptureProblem::SourceFailed {
                source: "game".into(),
                reason: reason.into(),
                explained: None
            })
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
            ended("Discord.exe", "it stopped delivering"),
            CaptureProblem::SourceEnded {
                source: "Discord.exe".into(),
                reason: "it stopped delivering".into(),
                explained: None
            }
        );
    }

    /// #296, block H: the headset unplugged mid-game. Plain words, no report,
    /// and the call and HRESULT kept in the reason.
    #[test]
    fn a_disconnected_device_is_said_plainly_and_asks_for_no_report() {
        let why = "GetNextPacketSize failed: 0x88890004";
        let mic = ended("microphone", why);
        assert_eq!(mic.reason(), why);
        assert_eq!(mic.told(), "the microphone was disconnected");
        assert!(!mic.wants_report());
        let desktop = ended(
            "desktop",
            "GetBuffer failed: The audio endpoint device has been invalidated. (0x88890004)",
        );
        assert_eq!(desktop.told(), "the output device was disconnected");
        assert!(!desktop.wants_report());
        // Lower case too, and at open as well as mid-recording.
        let at_open = not_opened(
            &AudioSourceKind::Microphone { device_id: None },
            &SourceError::open("IAudioClient::Initialize failed (0x88890004)"),
        )
        .unwrap();
        assert_eq!(at_open.told(), "the microphone was disconnected");
        assert_eq!(explain("game", "x (0x88890004)").map(|e| e.report), Some(false));
    }

    /// #296, block M2: the microphone blocked in Windows' privacy settings.
    #[test]
    fn a_blocked_microphone_says_where_the_setting_is() {
        let initialize = "IAudioClient::Initialize (the microphone) failed: Access is denied. (0x80070005)";
        let mic = AudioSourceKind::Microphone { device_id: Some("{0.0.1.00000000}.{abc}".into()) };
        let Some(problem) = not_opened(&mic, &SourceError::open(initialize)) else {
            panic!("a microphone that is there and refused is a failure");
        };
        assert_eq!(problem.reason(), initialize, "the stored reason keeps the call and HRESULT");
        assert_eq!(
            problem.explained(),
            Some(&Explained {
                text: "Windows is blocking microphone access".into(),
                fix: Some(
                    "Turn on \"Let desktop apps access your microphone\" in Settings → Privacy & \
                     security → Microphone."
                        .into()
                ),
                report: false,
            })
        );
    }

    /// Only the recognised codes, on the sources they mean something for: a
    /// refused process loopback is the Windows 10 case and stays a report.
    #[test]
    fn anything_else_keeps_its_reason_and_its_report() {
        let refused = "process-loopback activation for PID 4242 was refused: Access is denied. (0x80070005)";
        assert_eq!(explain("game", refused), None);
        assert_eq!(explain("Discord.exe", refused), None);
        assert_eq!(explain("microphone", "IAudioClient::Start failed (0x88890001)"), None);
        assert_eq!(explain("microphone", "no code at all"), None);
        // Not a prefix of a longer number, and not short of eight digits.
        assert_eq!(explain("microphone", "0x800700051"), None);
        assert_eq!(explain("microphone", "0x8007005"), None);
        assert_eq!(hresults("a (0x80070005) b 0X88890004 c 0xZZ"), vec![0x8007_0005, 0x8889_0004]);
    }

    #[test]
    fn a_stop_that_kept_a_file_early_says_why() {
        assert_eq!(stop_problem(StopAnswer::Clean), None);
        assert_eq!(
            stop_problem(StopAnswer::EndedEarly("the GPU device was lost (0x887A0005)")),
            Some(CaptureProblem::EndedEarly {
                reason: "the GPU device was lost (0x887A0005)".into(),
                explained: None
            })
        );
        let Some(CaptureProblem::EndedEarly { reason, .. }) =
            stop_problem(StopAnswer::FinalizeFailed("MF_E_X"))
        else {
            panic!()
        };
        assert!(reason.contains("MF_E_X") && reason.contains("last fragment"), "{reason}");
        let Some(problem) = stop_problem(StopAnswer::WorkerGone {
            why: Some("capture worker pid 7 exited with code 0xc0000005"),
            at_s: None,
        }) else {
            panic!()
        };
        assert!(problem.reason().contains("0xc0000005"), "{problem:?}");
        assert_eq!(problem.told(), "the capture worker stopped unexpectedly mid-recording");
        assert!(stop_problem(StopAnswer::WorkerGone { why: None, at_s: None }).is_some());
    }

    /// A worker that died says where the file it left ends (#299), which is
    /// where the recording stopped. The person is told it stopped
    /// unexpectedly; the pid and exit code are for the log and the stored
    /// diagnostics (#296), and it is still worth a report.
    #[test]
    fn a_dead_worker_says_where_the_recording_stopped() {
        let Some(problem) = stop_problem(StopAnswer::WorkerGone {
            why: Some("capture worker pid 7 exited with code 1"),
            at_s: Some(178.9),
        }) else {
            panic!()
        };
        assert_eq!(
            problem.reason(),
            "the capture worker stopped at 2:58: capture worker pid 7 exited with code 1"
        );
        assert_eq!(problem.told(), "the capture worker stopped unexpectedly at 2:58");
        assert!(problem.wants_report());
        let Some(problem) = stop_problem(StopAnswer::WorkerGone { why: None, at_s: Some(3723.0) })
        else {
            panic!()
        };
        assert_eq!(problem.reason(), "the capture worker stopped at 1:02:03");
        assert_eq!(problem.told(), "the capture worker stopped unexpectedly at 1:02:03");
    }

    #[test]
    fn the_clock_reads_like_the_players() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(59.99), "0:59");
        assert_eq!(clock(206.0), "3:26");
        assert_eq!(clock(3600.0), "1:00:00");
        assert_eq!(clock(-1.0), "0:00");
    }
}

