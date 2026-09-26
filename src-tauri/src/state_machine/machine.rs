//! The game state machine: pure state-transition logic, deliberately
//! separated from the async orchestration in `supervisor.rs`. Every
//! transition here is a plain function of (current state, event) → (new
//! state, actions), so the full edge-case matrix — crashes, reconnects,
//! dodges — is unit-testable without a real LCU or Live Client Data
//! connection. DEVELOPMENT.md §3.4.
//!
//! ```text
//! Idle ──(lockfile appears)──▶ ClientRunning
//! ClientRunning ──(phase: InProgress/Reconnect)──▶ WaitingForGame
//! WaitingForGame ──(live client data reachable)──▶ Recording
//! Recording ──(phase: EndOfGame | live client data gone)──▶ Finalizing
//! Recording ──(capture lost mid-game)──▶ WaitingForGame, which records
//!   again at the next poll (or, once the restarts are spent, waits the
//!   game out without polling)
//! Finalizing ──(finalize actions complete)──▶ ClientRunning (or Idle if
//!   the client vanished too)
//! ```

use crate::lcu::{GameflowPhase, LockfileInfo, LockfileState};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize, ts_rs::TS)]
pub enum GameState {
    Idle,
    ClientRunning,
    WaitingForGame,
    Recording,
    Finalizing,
}

#[derive(Debug, Clone)]
pub enum StateEvent {
    LockfileChanged(LockfileState),
    GameflowPhase(GameflowPhase),
    /// Live Client Data became reachable (a poll succeeded).
    LiveClientUp,
    /// Live Client Data stopped responding after having been reachable —
    /// the game process crashed or was force-closed. Not fired for the
    /// ordinary "not up yet" state before a game has loaded.
    LiveClientDown,
    /// Sent by the supervisor once it has finished executing the actions
    /// `Finalizing` was entered with (recorder stopped, polling/watchers
    /// torn down). Without this, `Finalizing` would have no way to leave
    /// on its own — DEVELOPMENT.md's diagram shows it as a transient
    /// processing state, not one that waits on further external input.
    FinalizeComplete,
    /// The capture backend lost the recording in flight while the game was
    /// still running: the own backend's capture worker died (#299). Sent by
    /// the supervisor as soon as the recorder reports it, not at the next
    /// stop, so the recording is finished and the state leaves `Recording`
    /// while the game is still going.
    CaptureLost,
}

/// How many times one game's recording is restarted after its capture was
/// lost (`StateEvent::CaptureLost`). Each restart is a fresh recording of
/// the rest of the game, the way a daemon restarted mid-game resumes one.
/// Bounded so that a capture that fails straight after every start (a
/// worker that crashes on the first frame, say) costs a few short files and
/// a few notifications, not one of each per second for the rest of the game.
pub const MAX_CAPTURE_RESTARTS: u8 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    StartGameflowWatch(LockfileInfo),
    StopGameflowWatch,
    StartLiveClientPoll,
    StopLiveClientPoll,
    StartRecording,
    StopRecording,
}

pub struct StateMachine {
    pub state: GameState,
    lockfile: Option<LockfileInfo>,
    /// Restarts left for the game in progress after a lost capture. Refilled
    /// whenever a new game begins (`ClientRunning` → `WaitingForGame`).
    capture_restarts_left: u8,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state: GameState::Idle,
            lockfile: None,
            capture_restarts_left: MAX_CAPTURE_RESTARTS,
        }
    }

    /// Phase gameflow reports when a game (or Practice Tool match) is
    /// actually running or being reconnected to. Chosen to match
    /// DEVELOPMENT.md's diagram; not verified live yet (no League client
    /// installed here — see docs/windows-verification.md). If live testing
    /// finds gameflow reports a distinct phase while merely spectating,
    /// that phase must NOT be added here, since DEVELOPMENT.md explicitly
    /// calls for spectator sessions to not trigger recording.
    fn is_game_running_phase(phase: &GameflowPhase) -> bool {
        matches!(phase, GameflowPhase::InProgress | GameflowPhase::Reconnect)
    }

    fn is_end_of_game_phase(phase: &GameflowPhase) -> bool {
        matches!(
            phase,
            GameflowPhase::EndOfGame
                | GameflowPhase::PreEndOfGame
                | GameflowPhase::WaitingForStats
        )
    }

    pub fn handle(&mut self, event: StateEvent) -> Vec<Action> {
        use GameState::*;

        match (&self.state, event) {
            // --- Idle ---------------------------------------------------
            (Idle, StateEvent::LockfileChanged(LockfileState::Present(info))) => {
                self.lockfile = Some(info.clone());
                self.state = ClientRunning;
                vec![Action::StartGameflowWatch(info)]
            }

            // --- ClientRunning -------------------------------------------
            (ClientRunning, StateEvent::LockfileChanged(LockfileState::Absent)) => {
                self.lockfile = None;
                self.state = Idle;
                vec![Action::StopGameflowWatch]
            }
            // Client restarted (new pid/port/password) without ever
            // reporting Absent in between — re-point the watcher.
            (ClientRunning, StateEvent::LockfileChanged(LockfileState::Present(info)))
                if self.lockfile.as_ref() != Some(&info) =>
            {
                self.lockfile = Some(info.clone());
                vec![Action::StopGameflowWatch, Action::StartGameflowWatch(info)]
            }
            (ClientRunning, StateEvent::GameflowPhase(phase))
                if Self::is_game_running_phase(&phase) =>
            {
                self.state = WaitingForGame;
                self.capture_restarts_left = MAX_CAPTURE_RESTARTS;
                vec![Action::StartLiveClientPoll]
            }

            // --- WaitingForGame -------------------------------------------
            (WaitingForGame, StateEvent::LockfileChanged(LockfileState::Absent)) => {
                // Client crashed before the game ever loaded in — nothing
                // was recorded, so there's nothing to finalize.
                self.lockfile = None;
                self.state = Idle;
                vec![Action::StopGameflowWatch, Action::StopLiveClientPoll]
            }
            (WaitingForGame, StateEvent::LiveClientUp) => {
                self.state = Recording;
                vec![Action::StartRecording]
            }
            // Gameflow bounced back out without the game ever loading in
            // (dodge, FailedToLaunch, etc.).
            (WaitingForGame, StateEvent::GameflowPhase(phase))
                if !Self::is_game_running_phase(&phase) =>
            {
                self.state = ClientRunning;
                vec![Action::StopLiveClientPoll]
            }

            // --- Recording ---------------------------------------------
            // Normal end of game, a crash (Live Client Data stops
            // responding), or the client itself disappearing all funnel
            // through the same finalize path so footage is handled
            // consistently regardless of trigger. The gameflow watch is
            // always stopped here and restarted (if the client's still
            // around) on `FinalizeComplete` below — simpler and safer than
            // tracking whether it needs restarting, at the cost of a brief
            // gap in gameflow watching during the short finalize window.
            (Recording, StateEvent::GameflowPhase(phase)) if Self::is_end_of_game_phase(&phase) => {
                self.state = Finalizing;
                vec![
                    Action::StopRecording,
                    Action::StopLiveClientPoll,
                    Action::StopGameflowWatch,
                ]
            }
            (Recording, StateEvent::LiveClientDown) => {
                self.state = Finalizing;
                vec![
                    Action::StopRecording,
                    Action::StopLiveClientPoll,
                    Action::StopGameflowWatch,
                ]
            }
            // The capture died under a game that is still running (#299).
            // The recording is finished now, with what reached the disk, and
            // the state goes back to `WaitingForGame` rather than through
            // `Finalizing`: the game has not ended, the gameflow watch and
            // the Live Client poll are both still wanted, and the next
            // successful poll (`LiveClientUp`) starts a fresh recording of the
            // rest of it, exactly as a daemon restarted mid-game would.
            //
            // Once this game's restarts are spent the poll is stopped too, so
            // nothing sends that `LiveClientUp`: the game is waited out in
            // `WaitingForGame`, not recorded, and the gameflow phase leaving
            // the game (or the client going) moves on from there as usual.
            (Recording, StateEvent::CaptureLost) => {
                self.state = WaitingForGame;
                if self.capture_restarts_left > 0 {
                    self.capture_restarts_left -= 1;
                    vec![Action::StopRecording]
                } else {
                    vec![Action::StopRecording, Action::StopLiveClientPoll]
                }
            }
            (Recording, StateEvent::LockfileChanged(LockfileState::Absent)) => {
                self.lockfile = None;
                self.state = Finalizing;
                vec![
                    Action::StopRecording,
                    Action::StopLiveClientPoll,
                    Action::StopGameflowWatch,
                ]
            }

            // --- Finalizing ----------------------------------------------
            // A lockfile change observed mid-finalize just updates our
            // record of it; the state transition waits for
            // `FinalizeComplete` so finalize work never gets interrupted
            // half-done.
            (Finalizing, StateEvent::LockfileChanged(new_state)) => {
                self.lockfile = match new_state {
                    LockfileState::Present(info) => Some(info),
                    LockfileState::Absent => None,
                };
                vec![]
            }
            (Finalizing, StateEvent::FinalizeComplete) => {
                if let Some(info) = self.lockfile.clone() {
                    self.state = ClientRunning;
                    vec![Action::StartGameflowWatch(info)]
                } else {
                    self.state = Idle;
                    vec![]
                }
            }

            // Anything else — a stale event arriving after we've already
            // moved on — is a no-op.
            _ => vec![],
        }
    }
}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lockfile_a() -> LockfileInfo {
        LockfileInfo::parse("LeagueClient:1:2999:pw-a:https").unwrap()
    }

    fn lockfile_b() -> LockfileInfo {
        LockfileInfo::parse("LeagueClient:2:3000:pw-b:https").unwrap()
    }

    #[test]
    fn happy_path_full_game_lifecycle() {
        let mut m = StateMachine::new();

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StartGameflowWatch(lockfile_a())]);

        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        assert_eq!(m.state, GameState::WaitingForGame);
        assert_eq!(actions, vec![Action::StartLiveClientPoll]);

        let actions = m.handle(StateEvent::LiveClientUp);
        assert_eq!(m.state, GameState::Recording);
        assert_eq!(actions, vec![Action::StartRecording]);

        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::EndOfGame));
        assert_eq!(m.state, GameState::Finalizing);
        assert_eq!(
            actions,
            vec![
                Action::StopRecording,
                Action::StopLiveClientPoll,
                Action::StopGameflowWatch
            ]
        );

        let actions = m.handle(StateEvent::FinalizeComplete);
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StartGameflowWatch(lockfile_a())]);
    }

    #[test]
    fn client_closing_while_idle_ish_returns_to_idle() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Absent));
        assert_eq!(m.state, GameState::Idle);
        assert_eq!(actions, vec![Action::StopGameflowWatch]);
    }

    #[test]
    fn client_restart_with_new_lockfile_repoints_gameflow_watch() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_b())));
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(
            actions,
            vec![Action::StopGameflowWatch, Action::StartGameflowWatch(lockfile_b())]
        );
    }

    #[test]
    fn same_lockfile_reported_again_is_a_no_op() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        assert_eq!(m.state, GameState::ClientRunning);
        assert!(actions.is_empty());
    }

    #[test]
    fn dodge_returns_to_client_running_without_recording() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        assert_eq!(m.state, GameState::WaitingForGame);

        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::Lobby));
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StopLiveClientPoll]);
    }

    #[test]
    fn client_crash_while_waiting_for_game_goes_straight_to_idle() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Absent));
        assert_eq!(m.state, GameState::Idle);
        assert_eq!(
            actions,
            vec![Action::StopGameflowWatch, Action::StopLiveClientPoll]
        );
    }

    fn enter_recording(m: &mut StateMachine) {
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        m.handle(StateEvent::LiveClientUp);
        assert_eq!(m.state, GameState::Recording);
    }

    #[test]
    fn game_crash_detected_via_live_client_down_preserves_footage() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);

        let actions = m.handle(StateEvent::LiveClientDown);
        assert_eq!(m.state, GameState::Finalizing);
        assert_eq!(
            actions,
            vec![
                Action::StopRecording,
                Action::StopLiveClientPoll,
                Action::StopGameflowWatch
            ]
        );

        let actions = m.handle(StateEvent::FinalizeComplete);
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StartGameflowWatch(lockfile_a())]);
    }

    #[test]
    fn client_crash_while_recording_finalizes_then_settles_on_idle() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);

        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Absent));
        assert_eq!(m.state, GameState::Finalizing);
        assert_eq!(
            actions,
            vec![
                Action::StopRecording,
                Action::StopLiveClientPoll,
                Action::StopGameflowWatch
            ]
        );

        // No lockfile to restart a gameflow watch against.
        let actions = m.handle(StateEvent::FinalizeComplete);
        assert_eq!(m.state, GameState::Idle);
        assert!(actions.is_empty());
    }

    #[test]
    fn client_restarting_mid_finalize_is_picked_up_by_finalize_complete() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);
        m.handle(StateEvent::LockfileChanged(LockfileState::Absent)); // client crashes mid-game
        assert_eq!(m.state, GameState::Finalizing);

        // Client comes back up (relaunched) before we've finished finalizing.
        let actions = m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_b())));
        assert_eq!(m.state, GameState::Finalizing, "must not leave Finalizing early");
        assert!(actions.is_empty());

        let actions = m.handle(StateEvent::FinalizeComplete);
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StartGameflowWatch(lockfile_b())]);
    }

    #[test]
    fn stale_events_after_moving_on_are_no_ops() {
        let mut m = StateMachine::new();
        // Idle, never touched — a stray gameflow update shouldn't do anything.
        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        assert_eq!(m.state, GameState::Idle);
        assert!(actions.is_empty());

        let actions = m.handle(StateEvent::LiveClientUp);
        assert_eq!(m.state, GameState::Idle);
        assert!(actions.is_empty());

        let actions = m.handle(StateEvent::FinalizeComplete);
        assert_eq!(m.state, GameState::Idle);
        assert!(actions.is_empty());
    }

    /// The capture worker died mid-game (#299): the recording is stopped at
    /// once and the state leaves `Recording`, but the game is still on, so
    /// the watchers stay up and the next poll records the rest of it.
    #[test]
    fn a_lost_capture_finishes_the_recording_and_waits_for_the_next_poll() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);

        let actions = m.handle(StateEvent::CaptureLost);
        assert_eq!(m.state, GameState::WaitingForGame);
        assert_eq!(actions, vec![Action::StopRecording]);

        // The blanket follow-up `dispatch` sends is a no-op here.
        assert!(m.handle(StateEvent::FinalizeComplete).is_empty());
        assert_eq!(m.state, GameState::WaitingForGame);

        let actions = m.handle(StateEvent::LiveClientUp);
        assert_eq!(m.state, GameState::Recording);
        assert_eq!(actions, vec![Action::StartRecording]);
    }

    /// A capture that keeps dying is restarted a bounded number of times per
    /// game; after that the poll is stopped, so nothing starts another.
    #[test]
    fn a_capture_that_keeps_dying_is_given_up_on_for_the_rest_of_the_game() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);
        for _ in 0..MAX_CAPTURE_RESTARTS {
            assert_eq!(m.handle(StateEvent::CaptureLost), vec![Action::StopRecording]);
            assert_eq!(m.handle(StateEvent::LiveClientUp), vec![Action::StartRecording]);
        }
        let actions = m.handle(StateEvent::CaptureLost);
        assert_eq!(m.state, GameState::WaitingForGame);
        assert_eq!(actions, vec![Action::StopRecording, Action::StopLiveClientPoll]);

        // The game going on reports nothing that moves the state...
        assert!(m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress)).is_empty());
        assert_eq!(m.state, GameState::WaitingForGame);
        // ...and its end returns to the client as a dodge would.
        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::EndOfGame));
        assert_eq!(m.state, GameState::ClientRunning);
        assert_eq!(actions, vec![Action::StopLiveClientPoll]);
    }

    /// The next game starts with its restarts refilled.
    #[test]
    fn the_next_game_gets_its_restarts_back() {
        let mut m = StateMachine::new();
        enter_recording(&mut m);
        for _ in 0..MAX_CAPTURE_RESTARTS {
            m.handle(StateEvent::CaptureLost);
            m.handle(StateEvent::LiveClientUp);
        }
        m.handle(StateEvent::CaptureLost);
        m.handle(StateEvent::GameflowPhase(GameflowPhase::EndOfGame));
        assert_eq!(m.state, GameState::ClientRunning);

        m.handle(StateEvent::GameflowPhase(GameflowPhase::InProgress));
        m.handle(StateEvent::LiveClientUp);
        assert_eq!(m.state, GameState::Recording);
        assert_eq!(m.handle(StateEvent::CaptureLost), vec![Action::StopRecording]);
    }

    /// A loss reported after the recording has already ended some other way
    /// (the game ended in the same moment) changes nothing.
    #[test]
    fn a_lost_capture_outside_recording_is_a_no_op() {
        let mut m = StateMachine::new();
        assert!(m.handle(StateEvent::CaptureLost).is_empty());
        assert_eq!(m.state, GameState::Idle);

        enter_recording(&mut m);
        m.handle(StateEvent::GameflowPhase(GameflowPhase::EndOfGame));
        assert_eq!(m.state, GameState::Finalizing);
        assert!(m.handle(StateEvent::CaptureLost).is_empty());
        assert_eq!(m.state, GameState::Finalizing);

        m.handle(StateEvent::FinalizeComplete);
        assert!(m.handle(StateEvent::CaptureLost).is_empty());
        assert_eq!(m.state, GameState::ClientRunning);
    }

    #[test]
    fn practice_tool_and_reconnect_both_trigger_waiting_for_game() {
        let mut m = StateMachine::new();
        m.handle(StateEvent::LockfileChanged(LockfileState::Present(lockfile_a())));
        let actions = m.handle(StateEvent::GameflowPhase(GameflowPhase::Reconnect));
        assert_eq!(m.state, GameState::WaitingForGame);
        assert_eq!(actions, vec![Action::StartLiveClientPoll]);
    }
}
