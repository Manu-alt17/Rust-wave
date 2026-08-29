//! Unified button-event boundary between the dedicated input-polling thread
//! and the firmware main loop.
//!
//! The polling thread only reads GPIOs (via `crate::buttons`) and pushes
//! here; the main loop is the sole consumer, draining one event per tick and
//! applying it through the normal UI path. This mirrors the BLE callback
//! boundary in `rustmix_remote::queue::RemoteEventQueue`.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use crate::buttons::ButtonEvent;

/// Larger than `RemoteEventQueue`'s capacity: remote page-turns are a
/// single-viewer "latest wins" stream, but button-menu navigation is a
/// discrete step counter where a human rapid-clicking Up/Down expects every
/// press to eventually land in order, so more headroom is given before any
/// drop policy kicks in.
pub const INPUT_EVENT_QUEUE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputEvent {
    Back,
    SelectLongPress,
    Button(ButtonEvent),
}

#[derive(Clone, Debug)]
pub struct InputEventQueue {
    inner: Arc<Mutex<VecDeque<InputEvent>>>,
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
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity.max(1)))),
            capacity: capacity.max(1),
        }
    }

    /// Push an event from the input-polling thread.
    ///
    /// If the queue is full, the oldest event is dropped. This is only
    /// reachable under sustained, pathological button mashing far beyond
    /// normal use.
    pub fn push(&self, event: InputEvent) {
        let mut inner = self.inner.lock().unwrap();
        if inner.len() >= self.capacity {
            inner.pop_front();
        }
        inner.push_back(event);
    }

    pub fn pop(&self) -> Option<InputEvent> {
        self.inner.lock().unwrap().pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
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
    fn queue_reports_length() {
        let queue = InputEventQueue::new(4);
        assert!(queue.is_empty());
        queue.push(InputEvent::Back);
        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());
    }
}
