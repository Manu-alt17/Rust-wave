//! Polling button adapters for the active-low onboard keys, GPIO0 BOOT back
//! action, and GPIO5 SELECT hold contextual action.

use core::fmt::Debug;

use anyhow::{anyhow, Result};
use embedded_hal::{delay::DelayNs, digital::InputPin};

const DEBOUNCE_MS: u32 = 10;
/// Hold duration required for GPIO5 SELECT to trigger a route-specific
/// contextual action instead of its normal short-press meaning.
pub const SELECT_LONG_PRESS_MS: u32 = 900;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ButtonEvent {
    Up,
    Select,
    Down,
}

/// Small polling adapter. The first milestone deliberately avoids interrupt
/// callbacks and global mutable state; the product UI can add an event queue
/// behind this interface later.
pub struct Buttons<UP, DOWN> {
    up: UP,
    down: DOWN,
}

impl<UP, DOWN> Buttons<UP, DOWN>
where
    UP: InputPin,
    UP::Error: Debug,
    DOWN: InputPin,
    DOWN::Error: Debug,
{
    #[must_use]
    pub fn new(up: UP, down: DOWN) -> Self {
        Self { up, down }
    }

    /// Return one debounced press. Keys are active low on the Waveshare board.
    pub fn poll<D: DelayNs>(&mut self, delay: &mut D) -> Result<Option<ButtonEvent>> {
        if self.is_pressed(Key::Up)? {
            return self.confirm(delay, Key::Up);
        }
        if self.is_pressed(Key::Down)? {
            return self.confirm(delay, Key::Down);
        }
        Ok(None)
    }

    fn confirm<D: DelayNs>(&mut self, delay: &mut D, key: Key) -> Result<Option<ButtonEvent>> {
        delay.delay_ms(DEBOUNCE_MS);
        if !self.is_pressed(key)? {
            return Ok(None);
        }

        // Do not generate repeated UI events while the panel is refreshing.
        // A single "not pressed" sample can be a brief contact glitch rather
        // than a real release, especially the longer the key stays physically
        // held, so confirm the release is stable before accepting it; a
        // spurious low->high blip must resume waiting, not be mistaken for a
        // release-then-new-press pair (which used to emit one extra event per
        // glitch, scaling with hold duration).
        loop {
            while self.is_pressed(key)? {
                delay.delay_ms(10);
            }
            delay.delay_ms(DEBOUNCE_MS);
            if !self.is_pressed(key)? {
                break;
            }
        }
        Ok(Some(key.into()))
    }

    fn is_pressed(&mut self, key: Key) -> Result<bool> {
        match key {
            Key::Up => self
                .up
                .is_low()
                .map_err(|error| anyhow!("GPIO4 UP read failed: {error:?}")),
            Key::Down => self
                .down
                .is_low()
                .map_err(|error| anyhow!("GPIO6 DOWN read failed: {error:?}")),
        }
    }
}

#[derive(Clone, Copy)]
enum Key {
    Up,
    Down,
}

impl From<Key> for ButtonEvent {
    fn from(key: Key) -> Self {
        match key {
            Key::Up => ButtonEvent::Up,
            Key::Down => ButtonEvent::Down,
        }
    }
}

/// Dedicated active-low GPIO0 BOOT-button adapter. A single debounced press
/// (any hold duration) always means hierarchical Back.
pub struct BootBackButton<BACK> {
    back: BACK,
}

impl<BACK> BootBackButton<BACK>
where
    BACK: InputPin,
    BACK::Error: Debug,
{
    #[must_use]
    pub fn new(back: BACK) -> Self {
        Self { back }
    }

    /// Return true once per confirmed BOOT press-and-release cycle.
    pub fn poll<D: DelayNs>(&mut self, delay: &mut D) -> Result<bool> {
        if !self.is_pressed()? {
            return Ok(false);
        }
        delay.delay_ms(DEBOUNCE_MS);
        if !self.is_pressed()? {
            return Ok(false);
        }

        // See Buttons::confirm: debounce the release too, not just the press,
        // so a mid-hold contact glitch can't split one physical press into
        // several confirmed events.
        loop {
            while self.is_pressed()? {
                delay.delay_ms(10);
            }
            delay.delay_ms(DEBOUNCE_MS);
            if !self.is_pressed()? {
                break;
            }
        }
        Ok(true)
    }

    fn is_pressed(&mut self) -> Result<bool> {
        self.back
            .is_low()
            .map_err(|error| anyhow!("GPIO0 BOOT read failed: {error:?}"))
    }
}

/// Dedicated active-low GPIO5 SELECT adapter that distinguishes a normal
/// short press (the existing confirm/open/type-key meaning, handled by the
/// caller like any other button event) from a held press, which route-
/// specific features (Sudoku axis selection, calendar agenda, on-screen
/// keyboard axis toggles) use for a contextual action without touching Back
/// behavior elsewhere.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectPressEvent {
    ShortPress,
    LongPress,
}

pub struct SelectHoldButton<SELECT> {
    select: SELECT,
}

impl<SELECT> SelectHoldButton<SELECT>
where
    SELECT: InputPin,
    SELECT::Error: Debug,
{
    #[must_use]
    pub fn new(select: SELECT) -> Self {
        Self { select }
    }

    /// Return one SELECT release classified as short or long.
    pub fn poll<D: DelayNs>(&mut self, delay: &mut D) -> Result<Option<SelectPressEvent>> {
        if !self.is_pressed()? {
            return Ok(None);
        }
        delay.delay_ms(DEBOUNCE_MS);
        if !self.is_pressed()? {
            return Ok(None);
        }

        let mut held_ms = DEBOUNCE_MS;
        loop {
            while self.is_pressed()? {
                if held_ms >= SELECT_LONG_PRESS_MS {
                    while self.is_pressed()? {
                        delay.delay_ms(10);
                    }
                    return Ok(Some(SelectPressEvent::LongPress));
                }
                delay.delay_ms(10);
                held_ms = held_ms.saturating_add(10);
            }

            // A "not pressed" sample here can be a brief contact glitch
            // rather than a real release (see Buttons::confirm); confirm it
            // is stable before reporting ShortPress, otherwise resume
            // counting so one long hold cannot fire more than one event.
            delay.delay_ms(DEBOUNCE_MS);
            if !self.is_pressed()? {
                return Ok(Some(SelectPressEvent::ShortPress));
            }
            held_ms = held_ms.saturating_add(DEBOUNCE_MS);
        }
    }

    fn is_pressed(&mut self) -> Result<bool> {
        self.select
            .is_low()
            .map_err(|error| anyhow!("GPIO5 SELECT read failed: {error:?}"))
    }
}
