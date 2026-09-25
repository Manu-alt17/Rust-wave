//! Software-side burst-sample capture around QMI8658 hardware tap events.
//!
//! This module itself is still diagnostic-only: it collects a short window
//! of raw accelerometer and gyroscope samples immediately before and after
//! each tap the hardware engine reports, so `TAP_AXIS`/`TAP_POLARITY` can be
//! cross-checked against fuller per-axis amplitude context, e.g. before any
//! zone-to-function mapping is designed (see `src/imu.rs`'s `TapStatus` doc
//! comment on why `TAP_AXIS` alone likely isn't enough to place a tap on the
//! screen). Nothing in this module drives navigation or any other product
//! action -- the single-tap/double-tap Reader page-turn feature reads
//! `TapStatus.kind` straight from `BoardServices::poll_tap_event` in
//! `main.rs`'s tap-poll loop, independent of the [`TapDiagnosticsSession`]
//! this module provides for that same loop's optional burst logging.

use crate::imu::{format_tenths, Axis3Tenths, TapAxis, TapKind, TapPolarity, TapStatus};

/// Toggle for the burst-sample logger (the noisy pre/post accel+gyro dumps);
/// flip to `true` again when tuning `TapConfig` needs that data. Off by
/// default now that the Reader tap-to-turn-page feature is in use -- that
/// feature does not depend on this flag (see `main.rs`'s tap-poll loop) and
/// keeps logging its own single-line `reader-tap-page-turn`/`-result`
/// records regardless.
pub const TAP_DIAGNOSTICS_ENABLED: bool = false;
/// Best-effort software poll cadence for the raw-sample ring buffer. This is
/// not synchronized to the QMI8658's internal accelerometer ODR -- the
/// hardware tap engine itself runs off that ODR regardless of how often the
/// main loop calls [`TapDiagnosticsSession::record_sample`], so the actual
/// achieved cadence (and therefore how far back `pre_samples` really reaches)
/// depends on how busy the main loop is; the logged sample timestamps make
/// that visible after the fact.
pub const TAP_DIAGNOSTICS_POLL_INTERVAL_MS: u64 = 15;
/// Samples retained before each detected tap.
const RING_CAPACITY: usize = 20;
/// Samples captured after each detected tap before its record is finalized.
const POST_CAPTURE_SAMPLES: usize = 8;

/// One raw accelerometer + gyroscope sample, timestamped against the same
/// monotonic clock `TapStatus` observations use.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RawSample {
    pub at_ms: u64,
    pub accel_mg_tenths: Axis3Tenths,
    pub gyro_dps_tenths: Axis3Tenths,
}

/// One finished tap record: the decoded `TAP_STATUS` fields plus the raw
/// sample burst captured around it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TapDiagnosticEvent {
    pub at_ms: u64,
    pub kind: TapKind,
    pub axis: TapAxis,
    pub polarity: TapPolarity,
    pub raw_status: u8,
    pub pre_samples: Vec<RawSample>,
    pub post_samples: Vec<RawSample>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingCapture {
    at_ms: u64,
    kind: TapKind,
    axis: TapAxis,
    polarity: TapPolarity,
    raw_status: u8,
    pre_samples: Vec<RawSample>,
    post_samples: Vec<RawSample>,
}

/// Fixed-capacity, allocation-free-per-push circular buffer of raw samples.
#[derive(Clone, Debug)]
struct RingBuffer {
    samples: [RawSample; RING_CAPACITY],
    next: usize,
    filled: usize,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self {
            samples: [RawSample::default(); RING_CAPACITY],
            next: 0,
            filled: 0,
        }
    }
}

impl RingBuffer {
    fn push(&mut self, sample: RawSample) {
        self.samples[self.next] = sample;
        self.next = (self.next + 1) % RING_CAPACITY;
        self.filled = (self.filled + 1).min(RING_CAPACITY);
    }

    /// Oldest-to-newest snapshot of everything currently retained.
    fn snapshot(&self) -> Vec<RawSample> {
        if self.filled < RING_CAPACITY {
            self.samples[..self.filled].to_vec()
        } else {
            let mut ordered = Vec::with_capacity(RING_CAPACITY);
            ordered.extend_from_slice(&self.samples[self.next..]);
            ordered.extend_from_slice(&self.samples[..self.next]);
            ordered
        }
    }
}

/// Stateful capture engine: feed it raw samples and confirmed tap events
/// (from [`crate::board_services::BoardServices::poll_tap_event`], which
/// already gates on the `STATUS1.TAP_EVENT` hardware edge -- see that
/// method's doc comment for why a raw `TAP_STATUS` comparison isn't used
/// here), get back finished [`TapDiagnosticEvent`] records once each capture
/// window closes.
#[derive(Clone, Debug, Default)]
pub struct TapDiagnosticsSession {
    ring: RingBuffer,
    pending: Option<PendingCapture>,
}

impl TapDiagnosticsSession {
    /// Push one raw accelerometer/gyroscope sample into the rolling
    /// pre-event window, and into a capture already in progress. Returns a
    /// finished record once a pending tap's post-event window fills.
    #[must_use]
    pub fn record_sample(&mut self, sample: RawSample) -> Option<TapDiagnosticEvent> {
        self.ring.push(sample);
        if let Some(pending) = self.pending.as_mut() {
            pending.post_samples.push(sample);
            if pending.post_samples.len() >= POST_CAPTURE_SAMPLES {
                let pending = self.pending.take().expect("checked Some above");
                return Some(TapDiagnosticEvent {
                    at_ms: pending.at_ms,
                    kind: pending.kind,
                    axis: pending.axis,
                    polarity: pending.polarity,
                    raw_status: pending.raw_status,
                    pre_samples: pending.pre_samples,
                    post_samples: pending.post_samples,
                });
            }
        }
        None
    }

    /// Feed one confirmed tap event (the caller must already have gated
    /// this on the `STATUS1.TAP_EVENT` hardware edge -- see the struct-level
    /// doc comment). Starts a burst capture, snapshotting the current ring
    /// buffer as `pre_samples`.
    ///
    /// An event arriving while a capture is already in progress is dropped
    /// rather than started; two taps closer together than the
    /// ~`POST_CAPTURE_SAMPLES` * poll-interval post-event window is a known
    /// gap in this diagnostic-only sampler, not a product requirement this
    /// phase needs to solve.
    pub fn observe_tap_status(&mut self, status: TapStatus, at_ms: u64) {
        if let (Some(kind), true) = (status.kind, self.pending.is_none()) {
            self.pending = Some(PendingCapture {
                at_ms,
                kind,
                axis: status.axis,
                polarity: status.polarity,
                raw_status: status.raw,
                pre_samples: self.ring.snapshot(),
                post_samples: Vec::with_capacity(POST_CAPTURE_SAMPLES),
            });
        }
    }
}

/// Render one burst window as a compact, semicolon-separated sequence of
/// `t<+/-offset>ms:x=..,y=..,z=..,gx=..,gy=..,gz=..` entries relative to
/// `event_at_ms`, for a single log line.
#[must_use]
pub fn compact_samples_label(samples: &[RawSample], event_at_ms: u64) -> String {
    samples
        .iter()
        .map(|sample| {
            let offset_ms = sample.at_ms as i64 - event_at_ms as i64;
            format!(
                "t{offset_ms:+}ms:x={},y={},z={},gx={},gy={},gz={}",
                format_tenths(sample.accel_mg_tenths.x),
                format_tenths(sample.accel_mg_tenths.y),
                format_tenths(sample.accel_mg_tenths.z),
                format_tenths(sample.gyro_dps_tenths.x),
                format_tenths(sample.gyro_dps_tenths.y),
                format_tenths(sample.gyro_dps_tenths.z),
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::{
        compact_samples_label, RawSample, TapDiagnosticsSession, POST_CAPTURE_SAMPLES,
        RING_CAPACITY,
    };
    use crate::imu::{Axis3Tenths, TapAxis, TapKind, TapPolarity, TapStatus};

    fn sample(at_ms: u64, x: i32) -> RawSample {
        RawSample {
            at_ms,
            accel_mg_tenths: Axis3Tenths {
                x,
                y: 0,
                z: 10_000,
            },
            gyro_dps_tenths: Axis3Tenths::default(),
        }
    }

    fn tap(raw: u8, kind: TapKind, axis: TapAxis, polarity: TapPolarity) -> TapStatus {
        TapStatus {
            kind: Some(kind),
            axis,
            polarity,
            raw,
        }
    }

    #[test]
    fn no_event_without_a_tap_status_edge() {
        let mut session = TapDiagnosticsSession::default();
        for i in 0..30 {
            assert_eq!(session.record_sample(sample(i, i as i32)), None);
        }
    }

    #[test]
    fn captures_pre_and_post_window_around_a_tap_edge() {
        let mut session = TapDiagnosticsSession::default();
        for i in 0..25 {
            assert_eq!(session.record_sample(sample(i, i as i32)), None);
        }

        session.observe_tap_status(
            tap(0x01, TapKind::Single, TapAxis::X, TapPolarity::Positive),
            25,
        );

        let mut finished = None;
        for i in 25..(25 + POST_CAPTURE_SAMPLES as u64) {
            if let Some(event) = session.record_sample(sample(i, i as i32)) {
                finished = Some(event);
            }
        }

        let event = finished.expect("post-capture window should close");
        assert_eq!(event.kind, TapKind::Single);
        assert_eq!(event.axis, TapAxis::X);
        assert_eq!(event.polarity, TapPolarity::Positive);
        assert_eq!(event.raw_status, 0x01);
        assert_eq!(event.pre_samples.len(), RING_CAPACITY);
        // Ring holds the most recent RING_CAPACITY samples pushed before the
        // edge was observed: samples 5..25 (25 pushed, capacity 20).
        assert_eq!(event.pre_samples.first().unwrap().at_ms, 5);
        assert_eq!(event.pre_samples.last().unwrap().at_ms, 24);
        assert_eq!(event.post_samples.len(), POST_CAPTURE_SAMPLES);
        assert_eq!(event.post_samples.first().unwrap().at_ms, 25);
    }

    #[test]
    fn a_confirmed_event_mid_capture_does_not_restart_it() {
        let mut session = TapDiagnosticsSession::default();
        session.observe_tap_status(
            tap(0x01, TapKind::Single, TapAxis::X, TapPolarity::Positive),
            0,
        );
        // Even a second confirmed event with the *same* decoded fields --
        // the case the STATUS1-edge gate upstream is specifically there to
        // still report correctly as two taps -- must not reset a capture
        // already in progress; it's simply dropped, per this module's known
        // limitation for back-to-back taps narrower than the post-capture
        // window.
        session.observe_tap_status(
            tap(0x01, TapKind::Single, TapAxis::X, TapPolarity::Positive),
            5,
        );

        let mut events = 0;
        for i in 0..(POST_CAPTURE_SAMPLES as u64) {
            if session.record_sample(sample(i, 0)).is_some() {
                events += 1;
            }
        }
        assert_eq!(events, 1);
    }

    #[test]
    fn compact_samples_label_marks_offsets_relative_to_the_event() {
        let samples = [sample(90, 12), sample(100, 34)];
        let label = compact_samples_label(&samples, 100);
        assert!(label.starts_with("t-10ms:x=1.2,"));
        assert!(label.contains(";t+0ms:x=3.4,"));
    }
}
