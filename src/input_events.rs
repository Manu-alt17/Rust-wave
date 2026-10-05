//! Unified button-event boundary between the dedicated input-polling thread
//! and the firmware main loop.
//!
//! The polling thread only reads GPIOs (via `crate::buttons`) and pushes
//! here; the main loop is the sole consumer, draining one event per tick and
//! applying it through the normal UI path.
//!
//! The main loop blocks in [`InputEventQueue::wait_timeout`] between
//! iterations, so a press wakes it immediately while an idle device can
//! sleep for long stretches (see `MAIN_LOOP_IDLE_WAIT_MS` in `main.rs`).

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use crate::{
    boot_profile::now_us,
    buttons::ButtonEvent,
    input_timing::{self, EventTiming},
};

/// Dropped events kept for the input timing diagnostic to report.
const MAX_DROPPED_KEPT: usize = 32;

/// Kept deliberately tiny: the e-paper redraw between presses is slow, so a
/// deep backlog makes the device keep turning pages / moving the cursor long
/// after the user stopped pressing. At most two presses are held; older ones
/// are dropped in favour of the most recent (see [`InputEventQueue::push`]).
pub const INPUT_EVENT_QUEUE_CAPACITY: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    Back,
    SelectLongPress,
    Button(ButtonEvent),
}

impl InputEvent {
    /// Short name used by the input timing report.
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::SelectLongPress => "select-long",
            Self::Button(ButtonEvent::Up) => "up",
            Self::Button(ButtonEvent::Down) => "down",
            Self::Button(ButtonEvent::Select) => "select",
        }
    }
}

#[derive(Debug, Default)]
struct QueueState {
    events: VecDeque<(InputEvent, EventTiming)>,
    /// Events dropped to make room, oldest first (diagnostic only).
    dropped: Vec<(InputEvent, EventTiming)>,
}

#[derive(Clone, Debug)]
pub struct InputEventQueue {
    inner: Arc<(Mutex<QueueState>, Condvar)>,
    capacity: usize,
}

impl Default for InputEventQueue {
    fn default() -> Self {
        Self::new(INPUT_EVENT_QUEUE_CAPACITY)
    }
}

impl InputEventQueue {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(QueueState {
                    events: VecDeque::with_capacity(capacity.max(1)),
                    dropped: Vec::new(),
                }),
                Condvar::new(),
            )),
            capacity: capacity.max(1),
        }
    }

    /// Push an event from the input-polling thread.
    ///
    /// If the queue is full, the oldest event is dropped, so rapid presses
    /// during a slow redraw collapse to the latest two.
    pub fn push(&self, event: InputEvent) {
        let now = if input_timing::ENABLED { now_us() } else { 0 };
        self.push_seen_at(event, now);
    }

    /// [`Self::push`] for the input timing diagnostic: `seen_us` is when the
    /// polling thread first read the key down, before debouncing it.
    pub fn push_seen_at(&self, event: InputEvent, seen_us: i64) {
        let timing = EventTiming {
            seen_us,
            queued_us: if input_timing::ENABLED { now_us() } else { 0 },
        };
        let (state, ready) = &*self.inner;
        let mut state = state.lock().unwrap();
        if state.events.len() >= self.capacity {
            let dropped = state.events.pop_front();
            if input_timing::ENABLED && state.dropped.len() < MAX_DROPPED_KEPT {
                state.dropped.extend(dropped);
            }
        }
        state.events.push_back((event, timing));
        ready.notify_all();
    }

    pub fn pop(&self) -> Option<InputEvent> {
        self.pop_timed().map(|(event, _)| event)
    }

    /// [`Self::pop`] with the event's timestamps.
    pub fn pop_timed(&self) -> Option<(InputEvent, EventTiming)> {
        self.inner.0.lock().unwrap().events.pop_front()
    }

    /// Events dropped since the last call, oldest first. Always empty
    /// unless the input timing diagnostic is on.
    pub fn take_dropped(&self) -> Vec<(InputEvent, EventTiming)> {
        std::mem::take(&mut self.inner.0.lock().unwrap().dropped)
    }

    /// Block until at least one event is queued or `timeout` elapses,
    /// whichever comes first; returns immediately when an event is already
    /// waiting. Returns whether an event is available. Spurious wakeups are
    /// absorbed, so an early return always means a real event.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let (state, ready) = &*self.inner;
        let state = state.lock().unwrap();
        let (state, _) = ready
            .wait_timeout_while(state, timeout, |state| state.events.is_empty())
            .unwrap();
        !state.events.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.0.lock().unwrap().events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_returns_events_in_order() {
        let queue = InputEventQueue::new(4);
        queue.push(InputEvent::Button(ButtonEvent::Up));
        queue.push(InputEvent::Button(ButtonEvent::Down));
        queue.push(InputEvent::Back);
        assert_eq!(queue.pop(), Some(InputEvent::Button(ButtonEvent::Up)));
        assert_eq!(queue.pop(), Some(InputEvent::Button(ButtonEvent::Down)));
        assert_eq!(queue.pop(), Some(InputEvent::Back));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn queue_drops_oldest_when_full() {
        let queue = InputEventQueue::new(2);
        queue.push(InputEvent::Button(ButtonEvent::Up));
        queue.push(InputEvent::Button(ButtonEvent::Down));
        queue.push(InputEvent::SelectLongPress);
        assert_eq!(queue.pop(), Some(InputEvent::Button(ButtonEvent::Down)));
        assert_eq!(queue.pop(), Some(InputEvent::SelectLongPress));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn wait_returns_immediately_when_an_event_is_already_queued() {
        let queue = InputEventQueue::new(4);
        queue.push(InputEvent::Back);
        let started = std::time::Instant::now();
        assert!(queue.wait_timeout(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn wait_times_out_on_an_empty_queue() {
        let queue = InputEventQueue::new(4);
        let started = std::time::Instant::now();
        assert!(!queue.wait_timeout(Duration::from_millis(30)));
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn a_push_from_another_thread_wakes_the_waiter_early() {
        let queue = InputEventQueue::new(4);
        let producer = queue.clone();
        let started = std::time::Instant::now();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            producer.push(InputEvent::Button(ButtonEvent::Select));
        });
        assert!(queue.wait_timeout(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(2));
        handle.join().unwrap();
        assert_eq!(queue.pop(), Some(InputEvent::Button(ButtonEvent::Select)));
    }

    #[test]
    fn queue_reports_length() {
        let queue = InputEventQueue::new(4);
        assert!(queue.is_empty());
        queue.push(InputEvent::Back);
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());
    }

    #[test]
    fn a_timed_event_keeps_when_the_key_was_first_seen() {
        let queue = InputEventQueue::new(2);
        queue.push_seen_at(InputEvent::Button(ButtonEvent::Down), 1_234);
        let (event, timing) = queue.pop_timed().unwrap();
        assert_eq!(event, InputEvent::Button(ButtonEvent::Down));
        assert_eq!(timing.seen_us, 1_234);
    }

    #[test]
    fn dropped_events_are_kept_for_the_diagnostic_only() {
        let queue = InputEventQueue::new(1);
        queue.push_seen_at(InputEvent::Button(ButtonEvent::Up), 10);
        queue.push_seen_at(InputEvent::Button(ButtonEvent::Down), 20);
        let dropped = queue.take_dropped();
        if input_timing::ENABLED {
            assert_eq!(dropped.len(), 1);
            assert_eq!(dropped[0].0, InputEvent::Button(ButtonEvent::Up));
            assert_eq!(dropped[0].1.seen_us, 10);
        } else {
            assert!(dropped.is_empty());
        }
        assert!(queue.take_dropped().is_empty());
        assert_eq!(queue.pop(), Some(InputEvent::Button(ButtonEvent::Down)));
    }

    #[test]
    fn every_event_has_a_report_name() {
        assert_eq!(InputEvent::Back.marker(), "back");
        assert_eq!(InputEvent::SelectLongPress.marker(), "select-long");
        assert_eq!(InputEvent::Button(ButtonEvent::Select).marker(), "select");
    }
}
