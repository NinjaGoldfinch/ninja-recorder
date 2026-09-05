//! Driving the state machine and the marker pipeline without League.
//!
//! `state_machine::machine`'s pure transition function has eleven unit
//! tests; `state_machine::supervisor` — the part that actually spawns
//! watchers, starts the recorder, and writes the finalize row — has two,
//! and has never run against a real LCU (DEVELOPMENT.md §3.4). Everything
//! here feeds that untested glue synthetic input through its real code
//! path: no mocks, no dry runs. Dispatching `GameflowPhase(InProgress)`
//! really does start the recorder.

use crate::live_client::AllGameData;
use crate::state_machine::{DevSessionView, StateEvent, SupervisorStatus};
use crate::{dev, lcu, AppState};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;

/// JSON-friendly mirror of `StateEvent`. The real enum carries
/// `LockfileState`/`GameflowPhase`, neither of which deserializes from
/// anything a form can produce — `GameflowPhase` in particular has a
/// hand-written `Deserialize` that takes a bare string, and `Unknown` is
/// its catch-all, so a typo would silently become a valid-but-inert phase
/// rather than an error the portal can show.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DevStateEvent {
    /// League Client appeared. Port and password are fabricated by default
    /// — the state machine only stores them to hand to the gameflow
    /// watcher, which will fail to connect and retry harmlessly.
    LockfilePresent {
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        password: Option<String>,
    },
    LockfileAbsent,
    GameflowPhase {
        phase: String,
    },
    LiveClientUp,
    LiveClientDown,
    FinalizeComplete,
}

impl DevStateEvent {
    fn into_state_event(self) -> StateEvent {
        match self {
            DevStateEvent::LockfilePresent { port, password } => {
                StateEvent::LockfileChanged(lcu::LockfileState::Present(lcu::LockfileInfo {
                    name: "LeagueClient".to_string(),
                    pid: 0,
                    port: port.unwrap_or(2999),
                    password: password.unwrap_or_else(|| "dev-portal".to_string()),
                    protocol: "https".to_string(),
                }))
            }
            DevStateEvent::LockfileAbsent => StateEvent::LockfileChanged(lcu::LockfileState::Absent),
            DevStateEvent::GameflowPhase { phase } => {
                StateEvent::GameflowPhase(lcu::GameflowPhase::from(phase.as_str()))
            }
            DevStateEvent::LiveClientUp => StateEvent::LiveClientUp,
            DevStateEvent::LiveClientDown => StateEvent::LiveClientDown,
            DevStateEvent::FinalizeComplete => StateEvent::FinalizeComplete,
        }
    }
}

#[derive(Serialize)]
pub struct DispatchReport {
    pub before: SupervisorStatus,
    pub after: SupervisorStatus,
    pub session: Option<DevSessionView>,
}

/// Feeds one event through the live supervisor. Reports the state on
/// either side so the portal can show the transition rather than just the
/// result — a dispatch that changes nothing is the interesting case.
#[tauri::command]
pub fn dev_dispatch_state_event(
    state: tauri::State<AppState>,
    app: tauri::AppHandle,
    event: DevStateEvent,
) -> Result<DispatchReport, String> {
    let before = state.supervisor.status();
    state.supervisor.dev_dispatch(event.into_state_event());
    let after = state.supervisor.status();

    // A transition out of Recording writes a row; anything else can't.
    if before.state != after.state {
        dev::notify_library_changed(&app);
    }

    Ok(DispatchReport {
        session: state.supervisor.dev_session_view(),
        before,
        after,
    })
}

#[derive(Serialize)]
pub struct InjectReport {
    pub accepted: bool,
    /// Why nothing happened, when `accepted` is false. Injecting outside a
    /// recording is the overwhelmingly common mistake, and it looks
    /// identical to a broken pipeline unless it's said out loud.
    pub note: Option<String>,
    pub markers_added: usize,
    pub samples_added: usize,
    pub session: Option<DevSessionView>,
    pub state: String,
}

/// Feeds one Live Client Data payload through the real marker and sample
/// pipeline, exactly as the 1 Hz poller would.
#[tauri::command]
pub fn dev_inject_snapshot(
    state: tauri::State<AppState>,
    snapshot: serde_json::Value,
) -> Result<InjectReport, String> {
    let parsed: AllGameData = serde_json::from_value(snapshot)
        .map_err(|e| format!("not a valid Live Client Data payload: {e}"))?;

    let before = state.supervisor.dev_session_view();
    state.supervisor.dev_on_snapshot(parsed);
    let after = state.supervisor.dev_session_view();

    let (markers_added, samples_added) = match (&before, &after) {
        (Some(b), Some(a)) => (
            a.marker_count.saturating_sub(b.marker_count),
            a.sample_count.saturating_sub(b.sample_count),
        ),
        (None, Some(a)) => (a.marker_count, a.sample_count),
        _ => (0, 0),
    };

    let status = state.supervisor.status();
    Ok(InjectReport {
        accepted: after.is_some(),
        note: after.is_none().then(|| {
            "No recording session is open, so the snapshot was discarded. Dispatch \
             LockfilePresent → GameflowPhase(InProgress) first — the supervisor only \
             collects markers between Recorder::start and Recorder::stop."
                .to_string()
        }),
        markers_added,
        samples_added,
        session: after,
        state: format!("{:?}", status.state),
    })
}

/// The in-flight session — markers and samples accumulating right now.
/// `game_state_status` only carries the *last finalized* recording, so
/// without this there is no way to watch the pipeline work.
#[tauri::command]
pub fn dev_session_snapshot(state: tauri::State<AppState>) -> Option<DevSessionView> {
    state.supervisor.dev_session_view()
}

// --- Scripted replay ---------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ReplaySpec {
    /// Base Live Client Data payload. Its `gameData.gameTime` and
    /// `events.Events` are rewritten each tick; everything else
    /// (`allPlayers`, `activePlayer`) is passed through, which is what
    /// makes `team_diff` produce a real curve.
    pub base_snapshot: serde_json::Value,
    /// Game seconds to simulate.
    pub duration_s: f64,
    /// Wall-clock speed multiplier. 60 means a 20-minute game finishes in
    /// 20 seconds.
    pub speed: f64,
    /// Scripted events spliced into the stream as game time passes them.
    #[serde(default)]
    pub events: Vec<ReplayEvent>,
    /// Dispatch the state events that start and stop a recording around
    /// the replay, so it finalizes into a real DB row and file.
    #[serde(default)]
    pub drive_state_machine: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReplayEvent {
    pub event_time: f64,
    pub event_name: String,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ReplayStatus {
    pub running: bool,
    pub game_time_s: f64,
    pub duration_s: f64,
    pub ticks: u64,
    pub events_fired: usize,
    pub finished: bool,
    pub error: Option<String>,
}

pub struct ReplayHandle {
    pub(crate) task: JoinHandle<()>,
    pub(crate) status: Arc<Mutex<ReplayStatus>>,
}

impl Drop for ReplayHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Plays a whole fake game. Each tick rewrites `gameTime` and the event
/// list, then pushes the payload through the same `on_snapshot` the poller
/// uses — so the `MarkerTracker`'s cross-poll de-duplication (each real
/// poll returns the *entire* event history, not just new events) is
/// exercised too, not bypassed.
#[tauri::command]
pub fn dev_replay_start(
    state: tauri::State<AppState>,
    dev: tauri::State<super::DevState>,
    app: tauri::AppHandle,
    spec: ReplaySpec,
) -> Result<(), String> {
    let mut slot = dev.replay.lock().map_err(|e| e.to_string())?;
    if slot.is_some() {
        return Err("a replay is already running — stop it first".to_string());
    }

    // Validate up front: a malformed base payload should fail the button
    // press, not a background task nobody is watching.
    serde_json::from_value::<AllGameData>(spec.base_snapshot.clone())
        .map_err(|e| format!("base_snapshot is not a valid Live Client Data payload: {e}"))?;
    if !spec.base_snapshot.is_object() {
        return Err("base_snapshot must be a JSON object".to_string());
    }
    let speed = if spec.speed > 0.0 { spec.speed } else { 1.0 };

    let status = Arc::new(Mutex::new(ReplayStatus {
        running: true,
        duration_s: spec.duration_s,
        ..Default::default()
    }));

    let supervisor = Arc::clone(&state.supervisor);
    let task_status = Arc::clone(&status);
    let app_handle = app.clone();

    let task = tauri::async_runtime::spawn(async move {
        if spec.drive_state_machine {
            supervisor.dev_dispatch(StateEvent::LockfileChanged(lcu::LockfileState::Present(
                lcu::LockfileInfo {
                    name: "LeagueClient".to_string(),
                    pid: 0,
                    port: 2999,
                    password: "dev-portal".to_string(),
                    protocol: "https".to_string(),
                },
            )));
            supervisor.dev_dispatch(StateEvent::GameflowPhase(lcu::GameflowPhase::InProgress));
        }

        // One tick per game second, wall-clock-scaled. Matching the real
        // poller's 1 Hz keeps the sample series the same density a live
        // game produces.
        let tick = Duration::from_secs_f64((1.0 / speed).clamp(0.001, 5.0));
        let total = spec.duration_s.max(0.0) as u64;

        for second in 0..=total {
            let game_time_s = second as f64;
            let fired: Vec<&ReplayEvent> = spec
                .events
                .iter()
                .filter(|e| e.event_time <= game_time_s)
                .collect();

            let mut payload = spec.base_snapshot.clone();
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    "gameData".to_string(),
                    serde_json::json!({ "gameTime": game_time_s }),
                );
                obj.insert(
                    "events".to_string(),
                    serde_json::json!({
                        "Events": fired
                            .iter()
                            .enumerate()
                            .map(|(i, e)| {
                                let mut v = serde_json::Map::new();
                                v.insert("EventID".to_string(), serde_json::json!(i));
                                v.insert("EventName".to_string(), serde_json::json!(e.event_name));
                                v.insert("EventTime".to_string(), serde_json::json!(e.event_time));
                                for (k, val) in &e.extra {
                                    v.insert(k.clone(), val.clone());
                                }
                                serde_json::Value::Object(v)
                            })
                            .collect::<Vec<_>>()
                    }),
                );
            }

            match serde_json::from_value::<AllGameData>(payload) {
                Ok(snapshot) => supervisor.dev_on_snapshot(snapshot),
                Err(e) => {
                    let mut s = task_status.lock().unwrap();
                    s.error = Some(e.to_string());
                    s.running = false;
                    return;
                }
            }

            {
                let mut s = task_status.lock().unwrap();
                s.game_time_s = game_time_s;
                s.ticks += 1;
                s.events_fired = fired.len();
            }

            tokio::time::sleep(tick).await;
        }

        if spec.drive_state_machine {
            supervisor.dev_dispatch(StateEvent::GameflowPhase(lcu::GameflowPhase::EndOfGame));
            supervisor.dev_emit_library_changed();
        }
        dev::notify_library_changed(&app_handle);

        let mut s = task_status.lock().unwrap();
        s.running = false;
        s.finished = true;
    });

    *slot = Some(ReplayHandle { task, status });
    Ok(())
}

#[tauri::command]
pub fn dev_replay_stop(dev: tauri::State<super::DevState>) -> Result<(), String> {
    // `ReplayHandle::drop` aborts the task.
    dev.replay.lock().map_err(|e| e.to_string())?.take();
    Ok(())
}

#[tauri::command]
pub fn dev_replay_status(dev: tauri::State<super::DevState>) -> Result<ReplayStatus, String> {
    let slot = dev.replay.lock().map_err(|e| e.to_string())?;
    Ok(match slot.as_ref() {
        Some(handle) => handle.status.lock().map_err(|e| e.to_string())?.clone(),
        None => ReplayStatus::default(),
    })
}

// --- Live API probes ---------------------------------------------------

/// Raw GET against any LCU path, so an endpoint can be inspected before
/// any parsing code is written for it. Returns the response as JSON.
#[tauri::command]
pub async fn dev_lcu_get(path: String) -> Result<serde_json::Value, String> {
    let lockfile = lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;
    let client = lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;
    client
        .get_json::<serde_json::Value>(&path)
        .await
        .map_err(|e| e.to_string())
}

/// Exercises `lcu::match_data::fetch_match_summary`, which is fully
/// implemented and unit-tested but called from nowhere in the app — which
/// is why every `RecordingRow`'s `champion`/`win`/`kda_*` is NULL in
/// practice. Wiring it into finalize is a separate change; this at least
/// makes it runnable against a real client.
#[tauri::command]
pub async fn dev_fetch_match_summary(game_id: i64) -> Result<lcu::MatchSummary, String> {
    let lockfile = lcu::lockfile::discover()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "League Client not running (no lockfile found)".to_string())?;
    let client = lcu::LcuHttpClient::new(&lockfile).map_err(|e| e.to_string())?;
    lcu::fetch_match_summary(&client, game_id)
        .await
        .map_err(|e| e.to_string())
}

/// One-shot fetch from the in-game Live Client Data API, returned raw so
/// it can be saved as a fixture.
#[tauri::command]
pub async fn dev_live_client_probe() -> Result<serde_json::Value, String> {
    let client = crate::live_client::LiveClientDataClient::new().map_err(|e| e.to_string())?;
    client
        .fetch_all_game_data_raw()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> DevStateEvent {
        serde_json::from_str(json).expect("valid dev state event")
    }

    #[test]
    fn maps_lockfile_events() {
        let StateEvent::LockfileChanged(lcu::LockfileState::Present(info)) =
            parse(r#"{"kind":"lockfile_present","port":1234}"#).into_state_event()
        else {
            panic!("expected a present lockfile");
        };
        assert_eq!(info.port, 1234);

        assert!(matches!(
            parse(r#"{"kind":"lockfile_absent"}"#).into_state_event(),
            StateEvent::LockfileChanged(lcu::LockfileState::Absent)
        ));
    }

    #[test]
    fn maps_known_gameflow_phases() {
        let StateEvent::GameflowPhase(phase) =
            parse(r#"{"kind":"gameflow_phase","phase":"InProgress"}"#).into_state_event()
        else {
            panic!("expected a gameflow phase");
        };
        assert_eq!(phase, lcu::GameflowPhase::InProgress);
    }

    /// A typo must stay visible as `Unknown(..)` rather than silently
    /// becoming a phase the state machine ignores — the portal shows the
    /// resulting state, and "nothing happened" needs an explanation.
    #[test]
    fn unknown_phase_names_round_trip_verbatim() {
        let StateEvent::GameflowPhase(phase) =
            parse(r#"{"kind":"gameflow_phase","phase":"InProgres"}"#).into_state_event()
        else {
            panic!("expected a gameflow phase");
        };
        assert_eq!(phase, lcu::GameflowPhase::Unknown("InProgres".to_string()));
    }

    #[test]
    fn maps_live_client_and_finalize_events() {
        assert!(matches!(
            parse(r#"{"kind":"live_client_up"}"#).into_state_event(),
            StateEvent::LiveClientUp
        ));
        assert!(matches!(
            parse(r#"{"kind":"live_client_down"}"#).into_state_event(),
            StateEvent::LiveClientDown
        ));
        assert!(matches!(
            parse(r#"{"kind":"finalize_complete"}"#).into_state_event(),
            StateEvent::FinalizeComplete
        ));
    }
}
