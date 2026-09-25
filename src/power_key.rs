//! AXP2101 PMIC power-key event interpretation.
//!
//! The Waveshare board routes the physical power button through the AXP2101
//! PMIC rather than through one of the three application-button GPIOs. The
//! register-level I2C access remains in [`crate::power`]; this module keeps the
//! short-press sleep and long-press menu product policy host-testable and
//! separate from PMIC transport.

/// Polling cadence for PMIC power-key status bits.
pub const POWER_KEY_POLL_MS: u64 = 100;

/// AXP2101 IRQ2 bit used for a POWERON long press.
///
/// XPowers names the source `XPOWERS_AXP2101_PKEY_LONG_IRQ` at global bit 10,
/// which maps to bit 2 of AXP2101 `INTEN2` / `INTSTS2`.
pub const POWER_KEY_LONG_PRESS_MASK: u8 = 1 << 2;

/// AXP2101 IRQ2 bit used for a POWERON short press.
///
/// XPowers names the source `XPOWERS_AXP2101_PKEY_SHORT_IRQ` at global bit 11,
/// which maps to bit 3 of AXP2101 `INTEN2` / `INTSTS2`.
pub const POWER_KEY_SHORT_PRESS_MASK: u8 = 1 << 3;

pub const POWER_KEY_EVENT_MASK: u8 = POWER_KEY_LONG_PRESS_MASK | POWER_KEY_SHORT_PRESS_MASK;

/// Minimum quiet interval after sleep-image entry before a new PMIC Power
/// event can wake the device. This suppresses queued PEK events emitted by the
/// same physical hold that initiated the sleep transition.
pub const POWER_KEY_WAKE_GUARD_QUIET_MS: u64 = 900;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepWakeGuardDecision {
    SuppressStalePress,
    AllowWake,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SleepWakeGuard {
    waiting_for_quiet_window: bool,
    armed: bool,
    suppressed_events: u32,
}

impl SleepWakeGuard {
    pub fn begin_sleep_entry(&mut self) {
        self.waiting_for_quiet_window = true;
        self.armed = false;
    }

    pub fn reset_after_wake(&mut self) {
        self.waiting_for_quiet_window = false;
        self.armed = false;
    }

    #[must_use]
    pub fn arm_after_quiet_window(&mut self, elapsed_ms: u64) -> bool {
        if self.waiting_for_quiet_window
            && !self.armed
            && elapsed_ms >= POWER_KEY_WAKE_GUARD_QUIET_MS
        {
            self.waiting_for_quiet_window = false;
            self.armed = true;
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn on_power_press(&mut self, elapsed_ms: u64) -> SleepWakeGuardDecision {
        let _ = self.arm_after_quiet_window(elapsed_ms);
        if self.armed {
            SleepWakeGuardDecision::AllowWake
        } else {
            self.suppressed_events = self.suppressed_events.saturating_add(1);
            SleepWakeGuardDecision::SuppressStalePress
        }
    }

    #[must_use]
    pub const fn suppressed_events(&self) -> u32 {
        self.suppressed_events
    }
}

/// Minimum time after boot before a PMIC Power-key event is acted on. The
/// physical press that just powered the board back on can still be latched
/// (or physically ongoing) when the event loop starts; `initialize_power_key_events`
/// already clears any stale `INTSTS2` bits before the loop begins, but this
/// guard is defense in depth against a fresh event landing in that same
/// narrow window and immediately shutting the board back down.
pub const POWER_KEY_BOOT_GUARD_QUIET_MS: u64 = 900;

/// Suppresses PMIC Power-key events for [`POWER_KEY_BOOT_GUARD_QUIET_MS`]
/// after boot. Unlike [`SleepWakeGuard`] (armed on entry into sleep, reset on
/// wake), this guard only ever needs to open once per boot and then stays
/// open, so it carries no re-arming state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BootPowerKeyGuard {
    armed: bool,
}

impl BootPowerKeyGuard {
    /// True while a Power-key event landing right now should be ignored.
    /// Once `elapsed_since_boot_ms` first reaches the quiet window the guard
    /// opens permanently, so later calls never need to keep re-checking the
    /// clock.
    #[must_use]
    pub fn should_ignore(&mut self, elapsed_since_boot_ms: u64) -> bool {
        if self.armed {
            false
        } else if elapsed_since_boot_ms < POWER_KEY_BOOT_GUARD_QUIET_MS {
            true
        } else {
            self.armed = true;
            false
        }
    }
}

/// Product-facing physical Power-key events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerKeyEvent {
    /// Enter the accepted sleep-image path while awake.
    ShortPress,
    /// Open the global display-maintenance menu while awake.
    LongPress,
}

impl PowerKeyEvent {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::ShortPress => "short-press",
            Self::LongPress => "long-press",
        }
    }
}

/// Interpret one AXP2101 `INTSTS2` byte. Long press wins when both sticky bits
/// are present so one held Power action cannot trigger the short-press sleep
/// transition first.
#[must_use]
pub const fn power_key_event_from_irq_status(status2: u8) -> Option<PowerKeyEvent> {
    if status2 & POWER_KEY_LONG_PRESS_MASK != 0 {
        Some(PowerKeyEvent::LongPress)
    } else if status2 & POWER_KEY_SHORT_PRESS_MASK != 0 {
        Some(PowerKeyEvent::ShortPress)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        power_key_event_from_irq_status, BootPowerKeyGuard, PowerKeyEvent, SleepWakeGuard,
        SleepWakeGuardDecision, POWER_KEY_BOOT_GUARD_QUIET_MS, POWER_KEY_EVENT_MASK,
        POWER_KEY_LONG_PRESS_MASK, POWER_KEY_SHORT_PRESS_MASK, POWER_KEY_WAKE_GUARD_QUIET_MS,
    };

    #[test]
    fn decodes_short_and_long_axp2101_power_key_bits_with_long_priority() {
        assert_eq!(POWER_KEY_LONG_PRESS_MASK, 0x04);
        assert_eq!(POWER_KEY_SHORT_PRESS_MASK, 0x08);
        assert_eq!(POWER_KEY_EVENT_MASK, 0x0C);
        assert_eq!(
            power_key_event_from_irq_status(0x08),
            Some(PowerKeyEvent::ShortPress)
        );
        assert_eq!(
            power_key_event_from_irq_status(0x04),
            Some(PowerKeyEvent::LongPress)
        );
        assert_eq!(
            power_key_event_from_irq_status(0x0C),
            Some(PowerKeyEvent::LongPress)
        );
    }

    #[test]
    fn ignores_unrelated_axp2101_irq2_bits() {
        assert_eq!(power_key_event_from_irq_status(0x00), None);
        assert_eq!(power_key_event_from_irq_status(0x10), None);
        assert_eq!(power_key_event_from_irq_status(0x80), None);
    }

    #[test]
    fn wake_guard_suppresses_entry_press_until_quiet_window() {
        let mut guard = SleepWakeGuard::default();
        guard.begin_sleep_entry();
        assert_eq!(POWER_KEY_WAKE_GUARD_QUIET_MS, 900);
        assert_eq!(
            guard.on_power_press(120),
            SleepWakeGuardDecision::SuppressStalePress
        );
        assert_eq!(guard.suppressed_events(), 1);
        assert!(!guard.arm_after_quiet_window(899));
        assert!(guard.arm_after_quiet_window(900));
        assert_eq!(guard.on_power_press(901), SleepWakeGuardDecision::AllowWake);
    }

    #[test]
    fn wake_guard_allows_first_press_after_elapsed_quiet_window() {
        let mut guard = SleepWakeGuard::default();
        guard.begin_sleep_entry();
        assert_eq!(
            guard.on_power_press(1_200),
            SleepWakeGuardDecision::AllowWake
        );
        guard.reset_after_wake();
        guard.begin_sleep_entry();
        assert_eq!(
            guard.on_power_press(0),
            SleepWakeGuardDecision::SuppressStalePress
        );
    }

    #[test]
    fn boot_guard_suppresses_events_until_quiet_window_then_stays_open() {
        let mut guard = BootPowerKeyGuard::default();
        assert_eq!(POWER_KEY_BOOT_GUARD_QUIET_MS, 900);
        assert!(guard.should_ignore(0));
        assert!(guard.should_ignore(899));
        assert!(!guard.should_ignore(900));
        // Once open, stays open even if later polled with a smaller elapsed
        // value than the quiet window (defensive against clock jitter).
        assert!(!guard.should_ignore(0));
    }

    #[test]
    fn boot_guard_allows_events_immediately_when_constructed_past_the_quiet_window() {
        let mut guard = BootPowerKeyGuard::default();
        assert!(!guard.should_ignore(1_500));
    }
}
