//! What an endpoint audio source does when its device goes away mid-recording
//! (#298): which failures it waits out, how often it tries the device again,
//! when a "Windows default" source moves to a new default, and what the gaps
//! it left amount to. Pure, so every case is a unit test; `own/win/audio`
//! makes the calls and asks.
//!
//! **Only an endpoint source reattaches**: the microphone, and the desktop
//! in loopback. A process-loopback source follows its process, and the game's
//! is restarted by the session when the game window comes back
//! (`AudioTracks::restart_game`).
//!
//! **What is reopened is what was asked for.** A configured microphone is
//! opened again by the endpoint id the picker stored; "Windows default" is
//! whatever the default for its role is *now* (the communications capture
//! device, or the console render device for the desktop), which after an
//! unplug is often another device. That is also why a default source
//! [`follows_default`]: a headset plugged back in becomes the default again,
//! and a source left on the fallback device would record nothing the user
//! hears.
//!
//! While the device is away its track is silence, which the mixer's
//! watermark writes as it does for any source that sends nothing. The thread
//! marks the first packet from the reopened device a discontinuity, so the
//! aligner places it where its stamp says rather than treating the gap as
//! drift.
//!
//! Microsoft's documentation for the calls this answers for.
//! `IAudioCaptureClient::GetNextPacketSize` lists
//! `AUDCLNT_E_DEVICE_INVALIDATED` ("the audio endpoint device has been
//! unplugged, or the audio hardware or associated hardware resources have
//! been reconfigured, disabled, removed, or otherwise made unavailable for
//! use"), `AUDCLNT_E_RESOURCES_INVALIDATED` and
//! `AUDCLNT_E_SERVICE_NOT_RUNNING`. "Recovering from an Invalid-Device Error"
//! gives the recovery this module decides: release the client, then for an
//! application that uses the default device call `GetDefaultAudioEndpoint`
//! for the current default and activate a client on it, and for one that
//! selects a specific device activate a client on the same device again.
//! <https://learn.microsoft.com/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getnextpacketsize>
//! <https://learn.microsoft.com/windows/win32/coreaudio/recovering-from-an-invalid-device-error>

use std::time::Duration;

/// `AUDCLNT_E_DEVICE_INVALIDATED`: the device was unplugged, disabled or
/// reconfigured.
pub const DEVICE_INVALIDATED: u32 = 0x8889_0004;
/// `AUDCLNT_E_SERVICE_NOT_RUNNING`: the audio service stopped, as it does
/// while it restarts.
pub const SERVICE_NOT_RUNNING: u32 = 0x8889_0010;
/// `AUDCLNT_E_RESOURCES_INVALIDATED`: "the stream's resources have been
/// invalidated" (a suspended stream, among others): a new stream is the way
/// back here too.
pub const RESOURCES_INVALIDATED: u32 = 0x8889_0026;

/// Whether a capture that failed with `hresult` is its device going away,
/// which is waited out, as against a failure that ends the source.
pub fn recoverable(hresult: Option<u32>) -> bool {
    matches!(hresult, Some(DEVICE_INVALIDATED | SERVICE_NOT_RUNNING | RESOURCES_INVALIDATED))
}

/// The first try comes half a second after the loss: long enough for Windows
/// to have chosen a new default, short enough that a default source falling
/// back to another device loses little.
pub const FIRST_TRY: Duration = Duration::from_millis(500);
/// Then once a second.
pub const RETRY: Duration = Duration::from_secs(1);
/// And once every two seconds after a minute away: a device that has not
/// come back by then is probably off for the game.
pub const SLOW_RETRY: Duration = Duration::from_secs(2);
pub const SLOW_AFTER: Duration = Duration::from_secs(60);
/// How often a "Windows default" source asks which device the default is.
pub const DEFAULT_CHECK: Duration = Duration::from_secs(1);

/// When the next attempt to reopen a lost device is due, measured from the
/// moment it was lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Retry {
    next: Duration,
    /// Attempts that failed so far.
    pub attempts: u32,
}

impl Default for Retry {
    fn default() -> Self {
        Retry::new()
    }
}

impl Retry {
    pub fn new() -> Retry {
        Retry { next: FIRST_TRY, attempts: 0 }
    }

    /// Whether an attempt is due `since` the loss.
    pub fn due(&self, since: Duration) -> bool {
        since >= self.next
    }

    /// An attempt made `since` the loss failed: the next is a step on from
    /// now, not from when it was due, so a slow attempt never makes the next
    /// one due at once.
    pub fn failed(&mut self, since: Duration) {
        self.attempts += 1;
        let step = if since < SLOW_AFTER { RETRY } else { SLOW_RETRY };
        self.next = since + step;
    }
}

/// Whether a "Windows default" source that opened endpoint `opened` should
/// move to `current`, what the default for its role is now. `None` (no
/// default at all, or the question failed) stays put: the source that is
/// running is better than none, and a device that is gone ends it anyway.
pub fn follows_default(opened: &str, current: Option<&str>) -> bool {
    current.is_some_and(|current| current != opened)
}

/// One stretch a source's device was away, on the performance counter in
/// 100 ns units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outage {
    /// When the capture failed.
    pub lost: i64,
    /// When a reopened device started capturing, or `None` if it never did
    /// before the recording stopped.
    pub back: Option<i64>,
    /// What the failing call said.
    pub reason: String,
}

impl Outage {
    /// How long the device was away, or `None` while it still is.
    pub fn length(&self) -> Option<Duration> {
        let back = self.back?;
        Some(Duration::from_nanos(u64::try_from(back - self.lost).unwrap_or(0) * 100))
    }
}

/// The silence every outage left, in total, counting one that never ended
/// up to `end`, also on the performance counter.
pub fn silent_for(outages: &[Outage], end: i64) -> Duration {
    outages
        .iter()
        .map(|o| {
            let back = o.back.unwrap_or(end).max(o.lost);
            Duration::from_nanos(u64::try_from(back - o.lost).unwrap_or(0) * 100)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 10_000;

    #[test]
    fn a_lost_device_is_waited_out_and_anything_else_ends_the_source() {
        assert!(recoverable(Some(0x8889_0004)));
        assert!(recoverable(Some(SERVICE_NOT_RUNNING)));
        assert!(recoverable(Some(RESOURCES_INVALIDATED)));
        // E_ACCESSDENIED, AUDCLNT_E_BUFFER_ERROR, and no HRESULT at all.
        assert!(!recoverable(Some(0x8007_0005)));
        assert!(!recoverable(Some(0x8889_0018)));
        assert!(!recoverable(None));
    }

    #[test]
    fn the_first_try_is_half_a_second_after_the_loss() {
        let retry = Retry::new();
        assert!(!retry.due(Duration::ZERO));
        assert!(!retry.due(Duration::from_millis(499)));
        assert!(retry.due(Duration::from_millis(500)));
    }

    #[test]
    fn a_failed_try_is_retried_each_second_then_every_two_after_a_minute() {
        let mut retry = Retry::new();
        retry.failed(Duration::from_millis(520));
        assert_eq!(retry.attempts, 1);
        assert!(!retry.due(Duration::from_millis(1_500)));
        assert!(retry.due(Duration::from_millis(1_520)));
        // A slow attempt: the next is a second on from when it finished.
        retry.failed(Duration::from_millis(2_900));
        assert!(!retry.due(Duration::from_millis(3_899)));
        assert!(retry.due(Duration::from_millis(3_900)));
        retry.failed(Duration::from_secs(60));
        assert!(!retry.due(Duration::from_millis(61_999)));
        assert!(retry.due(Duration::from_secs(62)));
        assert_eq!(retry.attempts, 3);
    }

    #[test]
    fn a_default_source_moves_when_the_default_does_and_not_otherwise() {
        let headset = "{0.0.1.00000000}.{headset}";
        let webcam = "{0.0.1.00000000}.{webcam}";
        assert!(follows_default(webcam, Some(headset)));
        assert!(!follows_default(headset, Some(headset)));
        // No default, or the question failed: stay on what is running.
        assert!(!follows_default(headset, None));
    }

    #[test]
    fn an_outage_is_as_long_as_the_device_was_away() {
        let back = Outage { lost: 8_000 * MS, back: Some(21_000 * MS), reason: "x".into() };
        assert_eq!(back.length(), Some(Duration::from_secs(13)));
        let gone = Outage { lost: 8_000 * MS, back: None, reason: "x".into() };
        assert_eq!(gone.length(), None);
        // One that never came back counts to the end.
        assert_eq!(silent_for(&[back.clone(), gone], 30_000 * MS), Duration::from_secs(13 + 22));
        assert_eq!(silent_for(&[back], 0), Duration::from_secs(13));
        assert_eq!(silent_for(&[], 0), Duration::ZERO);
    }
}
