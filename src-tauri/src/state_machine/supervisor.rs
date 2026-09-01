//! Async orchestration around the pure `StateMachine`: spawns/aborts the
//! lockfile, gameflow, and Live Client Data watchers per `Action`, and
//! drives the `Recorder` (Phase 1) and marker pipeline (`live_client`).
//! DEVELOPMENT.md §3.4.
//!
//! Unlike `machine.rs`, this glue can't be meaningfully unit-tested
//! without a real LCU/Live Client Data connection — no League client is
//! installed on the machine this was written on. It's kept as thin as
//! possible over the well-tested pure transition function specifically so
//! the untested surface is small: `execute` mostly just spawns/aborts
//! tasks and calls the already-tested `Recorder` trait methods.

use super::machine::{Action, GameState, StateEvent, StateMachine};
use crate::lcu;
use crate::live_client::{self, AllGameData, Marker, MarkerTracker, TimeAlignment};
use crate::recorder::{RecordConfig, Recorder};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::async_runtime::JoinHandle;

#[derive(Debug, Clone, Serialize)]
pub struct SessionMarker {
    #[serde(flatten)]
    pub marker: Marker,
    pub video_time_s: f64,
}

/// What the supervisor learned about the most recently finished recording,
/// surfaced to the frontend via `game_state_status`. Phase 4 will route
/// this into the SQLite VOD library instead of holding just the latest one
/// in memory.
#[derive(Debug, Clone, Serialize)]
pub struct FinalizedRecording {
    pub path: String,
    pub markers: Vec<SessionMarker>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupervisorStatus {
    pub state: GameState,
    pub last_finalized: Option<FinalizedRecording>,
}

struct RecordingSession {
    tracker: MarkerTracker,
    markers: Vec<SessionMarker>,
    alignment: Option<TimeAlignment>,
    record_started_at: Instant,
}

pub struct Supervisor {
    machine: Mutex<StateMachine>,
    recorder: Arc<Mutex<Box<dyn Recorder>>>,
    recordings_dir: PathBuf,
    gameflow_task: Mutex<Option<JoinHandle<()>>>,
    live_client_task: Mutex<Option<JoinHandle<()>>>,
    session: Mutex<Option<RecordingSession>>,
    last_finalized: Mutex<Option<FinalizedRecording>>,
}

impl Supervisor {
    pub fn new(recorder: Arc<Mutex<Box<dyn Recorder>>>, recordings_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            machine: Mutex::new(StateMachine::new()),
            recorder,
            recordings_dir,
            gameflow_task: Mutex::new(None),
            live_client_task: Mutex::new(None),
            session: Mutex::new(None),
            last_finalized: Mutex::new(None),
        })
    }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            state: self.machine.lock().unwrap().state.clone(),
            last_finalized: self.last_finalized.lock().unwrap().clone(),
        }
    }

    /// Starts the always-on lockfile watch. Everything else (gameflow
    /// watch, Live Client Data polling, recording) is started/stopped by
    /// state transitions from here on. Runs for the app's lifetime.
    pub fn start(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            lcu::lockfile::watch(Duration::from_secs(2), move |state| {
                sup.dispatch(StateEvent::LockfileChanged(state));
            })
            .await;
        });
    }

    /// Feeds one event through the state machine and executes whatever
    /// actions it returns, then always tries `FinalizeComplete` as a
    /// follow-up. That's only a real transition when we just entered
    /// `Finalizing` (whose finalize actions — stop recorder, stop
    /// pollers — run synchronously above, so completing immediately is
    /// correct); in every other state it hits the state machine's
    /// wildcard arm and is a no-op. This blanket approach means callers
    /// never need to remember to send `FinalizeComplete` themselves, and
    /// avoids re-entrant locking that a nested `dispatch` call from
    /// inside `execute` would cause.
    fn dispatch(self: &Arc<Self>, event: StateEvent) {
        self.dispatch_one(event);
        self.dispatch_one(StateEvent::FinalizeComplete);
    }

    fn dispatch_one(self: &Arc<Self>, event: StateEvent) {
        let actions = {
            let mut machine = self.machine.lock().unwrap();
            machine.handle(event)
        };
        for action in actions {
            self.execute(action);
        }
    }

    fn execute(self: &Arc<Self>, action: Action) {
        match action {
            Action::StartGameflowWatch(info) => self.start_gameflow_watch(info),
            Action::StopGameflowWatch => self.stop_gameflow_watch(),
            Action::StartLiveClientPoll => self.start_live_client_poll(),
            Action::StopLiveClientPoll => self.stop_live_client_poll(),
            Action::StartRecording => self.start_recording(),
            Action::StopRecording => self.stop_recording(),
        }
    }

    fn start_gameflow_watch(self: &Arc<Self>, lockfile: lcu::LockfileInfo) {
        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            let client = match lcu::LcuHttpClient::new(&lockfile) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[state_machine] failed to build LCU client: {e}");
                    return;
                }
            };
            lcu::gameflow::watch(&lockfile, &client, Duration::from_secs(1), {
                let sup = Arc::clone(&sup);
                move |update| sup.dispatch(StateEvent::GameflowPhase(update.phase))
            })
            .await;
        });
        *self.gameflow_task.lock().unwrap() = Some(handle);
    }

    fn stop_gameflow_watch(&self) {
        if let Some(handle) = self.gameflow_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    fn start_live_client_poll(self: &Arc<Self>) {
        let sup = Arc::clone(self);
        let handle = tauri::async_runtime::spawn(async move {
            let client = match live_client::LiveClientDataClient::new() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[state_machine] failed to build Live Client Data client: {e}");
                    return;
                }
            };
            live_client::poller::watch(
                &client,
                Duration::from_secs(1),
                {
                    let sup = Arc::clone(&sup);
                    move |snapshot| sup.on_snapshot(snapshot)
                },
                {
                    let sup = Arc::clone(&sup);
                    move || sup.dispatch(StateEvent::LiveClientDown)
                },
            )
            .await;
        });
        *self.live_client_task.lock().unwrap() = Some(handle);
    }

    fn stop_live_client_poll(&self) {
        if let Some(handle) = self.live_client_task.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Every successful poll: (1) tells the state machine Live Client Data
    /// is reachable — a no-op unless we're still `WaitingForGame`, in
    /// which case it starts recording; (2) if we're recording, extracts
    /// and time-aligns any markers new since the last poll.
    fn on_snapshot(self: &Arc<Self>, snapshot: AllGameData) {
        self.dispatch(StateEvent::LiveClientUp);

        let mut guard = self.session.lock().unwrap();
        let Some(session) = guard.as_mut() else {
            return;
        };

        if session.alignment.is_none() {
            let elapsed = session.record_started_at.elapsed().as_secs_f64();
            session.alignment = Some(TimeAlignment::new(snapshot.game_data.game_time, elapsed));
        }
        let alignment = session.alignment.expect("just set above if it was None");

        let fresh = session.tracker.ingest(&snapshot);
        session
            .markers
            .extend(fresh.into_iter().map(|marker| SessionMarker {
                video_time_s: alignment.video_time_s(marker.game_time_s),
                marker,
            }));
    }

    /// Executes `Action::StartRecording`. The state machine has already
    /// optimistically transitioned to `Recording` by the time this runs —
    /// if `Recorder::start` fails here (only reachable today via the dev
    /// panel's manual start button racing the automatic path, since
    /// nothing else calls it), the state machine's belief and the
    /// recorder's actual state diverge. Known gap, logged loudly rather
    /// than silently wrong; not expected to occur outside that manual
    /// double-start collision.
    fn start_recording(&self) {
        let config = RecordConfig {
            output_dir: self.recordings_dir.clone(),
            file_stem: format!("recording-{}", timestamp_millis()),
        };
        match self.recorder.lock().unwrap().start(config) {
            Ok(()) => {
                *self.session.lock().unwrap() = Some(RecordingSession {
                    tracker: MarkerTracker::new(),
                    markers: Vec::new(),
                    alignment: None,
                    record_started_at: Instant::now(),
                });
            }
            Err(e) => eprintln!("[state_machine] failed to start recording: {e}"),
        }
    }

    fn stop_recording(&self) {
        let session = self.session.lock().unwrap().take();
        match self.recorder.lock().unwrap().stop() {
            Ok(path) => {
                *self.last_finalized.lock().unwrap() = Some(FinalizedRecording {
                    path: path.display().to_string(),
                    markers: session.map(|s| s.markers).unwrap_or_default(),
                });
            }
            Err(e) => eprintln!("[state_machine] failed to stop recording: {e}"),
        }
    }
}

fn timestamp_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
