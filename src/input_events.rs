//! Unified button-event boundary between the dedicated input-polling thread
//! and the firmware main loop.
//!
//! The polling thread only reads GPIOs (via `crate::buttons`) and pushes
//! here; the main loop is the sole consumer, draining one event per tick and
//! applying it through the normal UI path. This mirrors the BLE callback
//! boundary in `rustmix_remote::queue::RemoteEventQueue`.
//!
//! The main loop blocks in [`InputEventQueue::wait_timeout`] between
//! iterations, so a press wakes it immediately while an idle device can
//! sleep for long stretches (see `MAIN_LOOP_IDLE_WAIT_MS` in `main.rs`).

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use crate::buttons::ButtonEvent;

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

#[derive(Clone, Debug)]
pub struct InputEventQueue {
    inner: Arc<(Mutex<VecDeque<InputEvent>>, Condvar)>,
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
                Mutex::new(VecDeque::with_capacity(capacity.max(1))),
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
        let (events, ready) = &*self.inner;
        let mut events = events.lock().unwrap();
        if events.len() >= self.capacity {
            events.pop_front();
        }
        events.push_back(event);
        ready.notify_all();
    }

    pub fn pop(&self) -> Option<InputEvent> {
        self.inner.0.lock().unwrap().pop_front()
    }

    /// Block until at least one event is queued or `timeout` elapses,
    /// whichever comes first; returns immediately when an event is already
    /// waiting. Returns whether an event is available. Spurious wakeups are
    /// absorbed, so an early return always means a real event.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let (events, ready) = &*self.inner;
        let events = events.lock().unwrap();
        let (events, _) = ready
            .wait_timeout_while(events, timeout, |events| events.is_empty())
            .unwrap();
        !events.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.0.lock().unwrap().len()
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
}
