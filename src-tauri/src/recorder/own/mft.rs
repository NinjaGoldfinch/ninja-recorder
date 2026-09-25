//! The bookkeeping of an **asynchronous** Media Foundation transform, with no
//! Windows in it (#239).
//!
//! A hardware H.264 encoder MFT is asynchronous: it says when it wants a frame
//! (`METransformNeedInput`) and when it has a sample ready
//! (`METransformHaveOutput`), one event each, and calling `ProcessInput`
//! without a matching `NeedInput` is an error (`MF_E_NOTACCEPTING`), as is
//! calling `ProcessOutput` without a `HaveOutput`. The session's cadence loop
//! does not wait on the encoder: it produces a frame every tick, and polls the
//! encoder's events without blocking on each pass. So frames and requests for
//! them arrive independently, and [`AsyncPump`] matches them up:
//!
//! - each `NeedInput` is one **credit**, spent on one `ProcessInput`;
//! - a frame that arrives with no credit waits in a **queue**, in order, and
//!   goes in as soon as a credit comes;
//! - each `HaveOutput` is one `ProcessOutput` owed;
//! - the queue is **bounded**: an encoder that stops asking for frames would
//!   otherwise hold every frame of the recording in memory, and the texture
//!   behind each one. A full queue is the recording's problem to report, not
//!   something to paper over by dropping frames.
//! - **draining**: once the last frame is in, the encoder is told to drain,
//!   `NeedInput` stops meaning anything, and `METransformDrainComplete` ends
//!   it.
//!
//! The software encoder is synchronous and needs none of this:
//! `ProcessInput` then `ProcessOutput` until it asks for more input
//! (`own/win/h264.rs`).
//!
//! Generic over the frame type so the tests need no COM object.

use std::collections::VecDeque;

/// An event from the transform's `IMFMediaEventGenerator`, as far as the
/// encoder's driving cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    NeedInput,
    HaveOutput,
    DrainComplete,
    /// Anything else (`METransformMarker`, an error event is handled by its
    /// status before it gets here): ignored.
    Other,
}

/// Credits, owed outputs and the frames waiting for a credit.
#[derive(Debug)]
pub struct AsyncPump<T> {
    credits: u32,
    outputs: u32,
    queue: VecDeque<T>,
    limit: usize,
    draining: bool,
    drained: bool,
    /// The deepest the queue has been, for the log line at stop.
    pub deepest: usize,
}

impl<T> AsyncPump<T> {
    /// A pump whose queue holds at most `limit` frames.
    pub fn new(limit: usize) -> Self {
        AsyncPump {
            credits: 0,
            outputs: 0,
            queue: VecDeque::new(),
            limit: limit.max(1),
            draining: false,
            drained: false,
            deepest: 0,
        }
    }

    /// Records one event.
    pub fn event(&mut self, event: Event) {
        match event {
            // Once draining, the encoder may still ask; nothing will come.
            Event::NeedInput if !self.draining => self.credits += 1,
            Event::NeedInput | Event::Other => {}
            Event::HaveOutput => self.outputs += 1,
            Event::DrainComplete => self.drained = true,
        }
    }

    /// Queues a frame. Hands it back if the queue is full, or if the encoder
    /// is already draining and will take nothing more.
    pub fn submit(&mut self, frame: T) -> Result<(), T> {
        if self.draining || self.queue.len() >= self.limit {
            return Err(frame);
        }
        self.queue.push_back(frame);
        self.deepest = self.deepest.max(self.queue.len());
        Ok(())
    }

    /// The next frame to hand to `ProcessInput`, spending a credit, or `None`
    /// if there is no frame or no credit.
    pub fn next_input(&mut self) -> Option<T> {
        if self.credits == 0 {
            return None;
        }
        let frame = self.queue.pop_front()?;
        self.credits -= 1;
        Some(frame)
    }

    /// Whether a `ProcessOutput` is owed, counting it as done.
    pub fn next_output(&mut self) -> bool {
        let owed = self.outputs > 0;
        self.outputs = self.outputs.saturating_sub(1);
        owed
    }

    /// Frames waiting for a credit.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Everything submitted has gone in: the drain can be asked for.
    pub fn ready_to_drain(&self) -> bool {
        self.queue.is_empty()
    }

    /// The drain has been asked for: nothing more goes in, and credits no
    /// longer accumulate.
    pub fn start_drain(&mut self) {
        self.draining = true;
        self.credits = 0;
    }

    pub fn draining(&self) -> bool {
        self.draining
    }

    /// `METransformDrainComplete` has arrived: every output is out.
    pub fn drained(&self) -> bool {
        self.drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_waits_for_a_credit_and_goes_in_order() {
        let mut pump = AsyncPump::new(8);
        pump.submit(1).unwrap();
        pump.submit(2).unwrap();
        assert_eq!(pump.next_input(), None, "no NeedInput yet");
        pump.event(Event::NeedInput);
        assert_eq!(pump.next_input(), Some(1));
        assert_eq!(pump.next_input(), None, "one credit, one frame");
        pump.event(Event::NeedInput);
        pump.event(Event::NeedInput);
        assert_eq!(pump.next_input(), Some(2));
        // A credit with nothing queued is kept for the next frame.
        assert_eq!(pump.next_input(), None);
        pump.submit(3).unwrap();
        assert_eq!(pump.next_input(), Some(3));
        assert_eq!(pump.next_input(), None);
        assert_eq!(pump.deepest, 2);
    }

    /// Credits banked while no frame was ready are each spent once.
    #[test]
    fn banked_credits_are_spent_once_each() {
        let mut pump = AsyncPump::new(8);
        for _ in 0..3 {
            pump.event(Event::NeedInput);
        }
        for f in 0..5 {
            pump.submit(f).unwrap();
        }
        let went: Vec<i32> = std::iter::from_fn(|| pump.next_input()).collect();
        assert_eq!(went, vec![0, 1, 2]);
        assert_eq!(pump.queued(), 2);
    }

    #[test]
    fn each_have_output_is_one_process_output() {
        let mut pump: AsyncPump<()> = AsyncPump::new(1);
        assert!(!pump.next_output());
        pump.event(Event::HaveOutput);
        pump.event(Event::HaveOutput);
        pump.event(Event::Other);
        assert!(pump.next_output());
        assert!(pump.next_output());
        assert!(!pump.next_output());
    }

    /// An encoder that stops asking fills the queue, and the frame that does
    /// not fit comes back rather than being dropped silently.
    #[test]
    fn a_full_queue_hands_the_frame_back() {
        let mut pump = AsyncPump::new(2);
        pump.submit(1).unwrap();
        pump.submit(2).unwrap();
        assert_eq!(pump.submit(3), Err(3));
        pump.event(Event::NeedInput);
        assert_eq!(pump.next_input(), Some(1));
        pump.submit(3).unwrap();
    }

    /// The end of a recording: the queue empties, the drain is asked for, a
    /// late NeedInput is ignored, outputs still come, and DrainComplete ends
    /// it.
    #[test]
    fn draining_takes_nothing_more_and_ends_on_drain_complete() {
        let mut pump = AsyncPump::new(4);
        pump.submit(1).unwrap();
        assert!(!pump.ready_to_drain());
        pump.event(Event::NeedInput);
        assert_eq!(pump.next_input(), Some(1));
        assert!(pump.ready_to_drain());
        pump.event(Event::NeedInput);
        pump.start_drain();
        assert!(pump.draining());
        pump.event(Event::NeedInput);
        assert_eq!(pump.submit(2), Err(2));
        assert_eq!(pump.next_input(), None);
        pump.event(Event::HaveOutput);
        assert!(pump.next_output());
        assert!(!pump.drained());
        pump.event(Event::DrainComplete);
        assert!(pump.drained());
    }

    #[test]
    fn a_zero_limit_still_holds_one_frame() {
        let mut pump = AsyncPump::new(0);
        pump.submit(1).unwrap();
        assert_eq!(pump.submit(2), Err(2));
    }
}
