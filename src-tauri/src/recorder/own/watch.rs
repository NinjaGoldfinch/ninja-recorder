//! Whether the game window the recording captures is still there, and when
//! to look for a new one, with no Windows in it (#302).
//!
//! **Why this polls.** WGC's `GraphicsCaptureItem.Closed` was meant to say
//! the window had gone, and on the box it never fired: not on a normal game
//! end, not with `League of Legends.exe` killed in Task Manager. So the
//! session thread asks Windows itself, every [`CHECK_EVERY`], whether the
//! window it captures still exists (`IsWindow`) and still belongs to the
//! process it did at attach (`GetWindowThreadProcessId`). The second question
//! is there because [Microsoft's `IsWindow` page][iswindow] warns that window
//! handles are recycled: a handle that names someone else's window now is a
//! window that has gone. `Closed` is still listened to, as a second signal.
//!
//! **Why it looks again.** After a crash the client offers Reconnect, and the
//! state machine stays in Recording through it, so the recording carries on
//! while the game restarts. Once the window has gone the loop writes black,
//! and every [`SEARCH_EVERY`] it looks for a game window, the same lookup
//! `start` used; one that is there, has a size and belongs to
//! `League of Legends.exe` is captured again ([`Watch::consider`]). A normal
//! game end finds nothing, and the black runs to `stop` as before.
//!
//! The session (`win/session.rs`) makes the calls; this decides when to make
//! them and what the answers mean.
//!
//! [iswindow]: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindow

use std::time::{Duration, Instant};

/// How often the loop asks whether the window still exists. Two cheap
/// calls, but the loop turns every millisecond, so not every pass. A quarter
/// of a second of frozen frame before the black is not worth finding sooner.
pub const CHECK_EVERY: Duration = Duration::from_millis(250);

/// How often a loop that has lost its window looks for a new one. The lookup
/// is a `FindWindowW`, plus a process snapshot when it finds something, and
/// a reconnecting game takes tens of seconds to come back.
pub const SEARCH_EVERY: Duration = Duration::from_secs(1);

/// How many reattaches a recording logs, with the close before each. A
/// window that WGC keeps closing while it stays up would otherwise be two
/// lines a second, since the search finds it again each time.
pub const LOGGED_REATTACHES: u32 = 10;

/// A game window: its handle as a number, and the process that owned it
/// when it was attached, if that could be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub handle: isize,
    pub owner: Option<u32>,
}

/// What the loop should ask Windows on this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    Nothing,
    /// Whether the attached window is still there ([`gone`]).
    Check,
    /// Whether a game window has appeared ([`Watch::consider`]).
    Search,
}

/// How the loop learned that the window had gone, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// The poll: the handle names no window, or someone else's.
    Destroyed,
    /// WGC raised `Closed`.
    Closed,
}

impl Lost {
    pub fn describe(self) -> &'static str {
        match self {
            Lost::Destroyed => "the window no longer exists",
            Lost::Closed => "WGC raised Closed",
        }
    }
}

/// What a search found: the window `FindWindowW` returned, whether it has a
/// client area worth capturing, and whether its owner is the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub window: Window,
    pub sized: bool,
    pub game: bool,
}

/// What to do with a search's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to capture yet: keep black, keep looking.
    Wait,
    /// Capture this window.
    Attach(Window),
}

/// Whether the window attached as owned by `owner_then` has gone, given what
/// Windows says now: `alive` from `IsWindow`, `owner_now` from
/// `GetWindowThreadProcessId` (which answers nothing for a destroyed window).
///
/// A handle that is alive but owned by a different process has been
/// recycled, so it has gone too. An owner that could not be read at attach
/// cannot be compared, and then `IsWindow` alone decides.
pub fn gone(alive: bool, owner_now: Option<u32>, owner_then: Option<u32>) -> bool {
    if !alive {
        return true;
    }
    match owner_then {
        Some(then) => owner_now != Some(then),
        None => false,
    }
}

/// The window the recording is attached to, whether it is still there, and
/// when to ask next.
pub struct Watch {
    window: Window,
    lost: Option<Lost>,
    next: Instant,
    /// The window the last attach failed on, so a window that keeps failing
    /// is logged once rather than every second.
    failed: Option<Window>,
    /// Windows attached after the first: the reattaches.
    reattached: u32,
}

impl Watch {
    /// Watching `window`, attached at `now`.
    pub fn new(window: Window, now: Instant) -> Watch {
        Watch { window, lost: None, next: now + CHECK_EVERY, failed: None, reattached: 0 }
    }

    /// The window last attached, gone or not.
    pub fn window(&self) -> Window {
        self.window
    }

    /// Whether the attached window has gone, and so the loop writes black.
    pub fn is_lost(&self) -> bool {
        self.lost.is_some()
    }

    /// How many times a new window has been attached since the first.
    pub fn reattached(&self) -> u32 {
        self.reattached
    }

    /// Whether the next close and return are still logged
    /// ([`LOGGED_REATTACHES`]).
    pub fn logging(&self) -> bool {
        self.reattached < LOGGED_REATTACHES
    }

    /// Whether the reattach just made is the last one logged, so its line
    /// can say the rest are not.
    pub fn last_logged(&self) -> bool {
        self.reattached == LOGGED_REATTACHES
    }

    /// What to ask on the pass at `now`. At most one question per interval:
    /// asking schedules the next.
    pub fn due(&mut self, now: Instant) -> Due {
        if now < self.next {
            return Due::Nothing;
        }
        if self.lost.is_some() {
            self.next = now + SEARCH_EVERY;
            Due::Search
        } else {
            self.next = now + CHECK_EVERY;
            Due::Check
        }
    }

    /// The window has gone, learned at `now` in the way `how` says. Returns
    /// `how` the first time, for the one log line, and `None` after: the
    /// poll and `Closed` can both report the same close. The first search is
    /// a whole [`SEARCH_EVERY`] later, which is sooner than any game restarts.
    pub fn lose(&mut self, how: Lost, now: Instant) -> Option<Lost> {
        if self.lost.is_some() {
            return None;
        }
        self.lost = Some(how);
        self.next = now + SEARCH_EVERY;
        self.failed = None;
        Some(how)
    }

    /// Whether to capture what a search found. Only while lost, and only a
    /// window with a size (a minimised one sends no frames) that belongs to
    /// the game: another window of the same class is not the game.
    ///
    /// The window can be the one that was lost. That is only possible when
    /// `Closed` said so and the window lived on, which is WGC ending the
    /// capture (Microsoft's `Closed` page says an app replacing its window
    /// does this); capturing it again is the right answer there too.
    pub fn consider(&self, found: Option<Candidate>) -> Verdict {
        match found {
            Some(c) if self.lost.is_some() && c.sized && c.game => Verdict::Attach(c.window),
            _ => Verdict::Wait,
        }
    }

    /// Capturing `window` from `now`: the black ends and the checks resume.
    pub fn attached(&mut self, window: Window, now: Instant) {
        self.window = window;
        self.lost = None;
        self.failed = None;
        self.next = now + CHECK_EVERY;
        self.reattached += 1;
    }

    /// Starting a capture of `window` failed. Returns whether to log it:
    /// the first failure on each window, not every second's retry.
    pub fn attach_failed(&mut self, window: Window) -> bool {
        let first = self.failed != Some(window);
        self.failed = Some(window);
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: Window = Window { handle: 0x1234, owner: Some(100) };
    const NEW_GAME: Window = Window { handle: 0x5678, owner: Some(200) };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_destroyed_window_has_gone() {
        assert!(gone(false, None, Some(100)));
        assert!(gone(false, Some(100), Some(100)));
        assert!(gone(false, None, None));
    }

    #[test]
    fn a_live_window_with_its_owner_has_not_gone() {
        assert!(!gone(true, Some(100), Some(100)));
    }

    #[test]
    fn a_recycled_handle_has_gone() {
        // The handle names a window again, but another process's.
        assert!(gone(true, Some(300), Some(100)));
        assert!(gone(true, None, Some(100)));
    }

    #[test]
    fn with_no_owner_at_attach_is_window_alone_decides() {
        assert!(!gone(true, Some(300), None));
        assert!(!gone(true, None, None));
    }

    #[test]
    fn checks_come_every_quarter_second_and_not_every_pass() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        assert_eq!(watch.due(t0), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(249)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(250)), Due::Check);
        // Asked: the next is a whole interval on.
        assert_eq!(watch.due(t0 + ms(251)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(499)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(500)), Due::Check);
    }

    #[test]
    fn a_late_pass_asks_once_rather_than_catching_up() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        assert_eq!(watch.due(t0 + ms(2000)), Due::Check);
        assert_eq!(watch.due(t0 + ms(2001)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(2250)), Due::Check);
    }

    #[test]
    fn once_lost_it_searches_every_second_instead_of_checking() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        assert_eq!(watch.lose(Lost::Destroyed, t0 + ms(300)), Some(Lost::Destroyed));
        assert!(watch.is_lost());
        assert_eq!(watch.due(t0 + ms(550)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(1299)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(1300)), Due::Search);
        assert_eq!(watch.due(t0 + ms(1800)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(2300)), Due::Search);
    }

    #[test]
    fn a_close_is_reported_once_whichever_signal_comes_first() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        assert_eq!(watch.lose(Lost::Closed, t0), Some(Lost::Closed));
        assert_eq!(watch.lose(Lost::Destroyed, t0 + ms(250)), None);
        assert_eq!(watch.lose(Lost::Closed, t0 + ms(500)), None);
    }

    #[test]
    fn nothing_found_waits() {
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Destroyed, Instant::now());
        assert_eq!(watch.consider(None), Verdict::Wait);
    }

    #[test]
    fn a_new_game_window_is_attached() {
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Destroyed, Instant::now());
        let found = Candidate { window: NEW_GAME, sized: true, game: true };
        assert_eq!(watch.consider(Some(found)), Verdict::Attach(NEW_GAME));
    }

    #[test]
    fn a_window_with_no_size_yet_waits() {
        // Still coming up, or minimised: WGC would send nothing.
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Destroyed, Instant::now());
        let found = Candidate { window: NEW_GAME, sized: false, game: true };
        assert_eq!(watch.consider(Some(found)), Verdict::Wait);
    }

    #[test]
    fn a_window_of_the_class_that_is_not_the_game_waits() {
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Destroyed, Instant::now());
        let found = Candidate { window: NEW_GAME, sized: true, game: false };
        assert_eq!(watch.consider(Some(found)), Verdict::Wait);
    }

    #[test]
    fn nothing_is_attached_while_the_window_is_still_there() {
        let watch = Watch::new(GAME, Instant::now());
        let found = Candidate { window: NEW_GAME, sized: true, game: true };
        assert_eq!(watch.consider(Some(found)), Verdict::Wait);
    }

    #[test]
    fn the_same_window_is_captured_again_after_closed_alone() {
        // `Closed` fired and the window lived on: WGC ended the capture.
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Closed, Instant::now());
        let found = Candidate { window: GAME, sized: true, game: true };
        assert_eq!(watch.consider(Some(found)), Verdict::Attach(GAME));
    }

    #[test]
    fn attaching_ends_the_black_and_resumes_the_checks() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        watch.lose(Lost::Destroyed, t0);
        watch.attached(NEW_GAME, t0 + ms(5000));
        assert!(!watch.is_lost());
        assert_eq!(watch.window(), NEW_GAME);
        assert_eq!(watch.reattached(), 1);
        assert_eq!(watch.due(t0 + ms(5100)), Due::Nothing);
        assert_eq!(watch.due(t0 + ms(5250)), Due::Check);
        // And the new window can be lost in turn, and reported again.
        assert_eq!(watch.lose(Lost::Destroyed, t0 + ms(6000)), Some(Lost::Destroyed));
    }

    #[test]
    fn closes_and_returns_are_logged_up_to_a_limit() {
        let t0 = Instant::now();
        let mut watch = Watch::new(GAME, t0);
        for n in 1..=LOGGED_REATTACHES {
            assert!(watch.logging(), "close {n} is logged");
            watch.lose(Lost::Closed, t0);
            watch.attached(GAME, t0);
            assert_eq!(watch.last_logged(), n == LOGGED_REATTACHES);
        }
        assert!(!watch.logging());
        watch.lose(Lost::Closed, t0);
        watch.attached(GAME, t0);
        assert!(!watch.last_logged());
    }

    #[test]
    fn a_failing_window_is_logged_once_and_a_different_one_again() {
        let mut watch = Watch::new(GAME, Instant::now());
        watch.lose(Lost::Destroyed, Instant::now());
        assert!(watch.attach_failed(NEW_GAME));
        assert!(!watch.attach_failed(NEW_GAME));
        assert!(!watch.attach_failed(NEW_GAME));
        let other = Window { handle: 0x9abc, owner: Some(200) };
        assert!(watch.attach_failed(other));
        assert!(watch.attach_failed(NEW_GAME));
    }
}
