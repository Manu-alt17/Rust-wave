//! AXP2101 support for panel power and sample-app battery status.
//!
//! The verified panel contract remains narrow: enable `ALDO3` before panel
//! initialization and disable it after deep sleep. The v0.4.0 sample-app slice
//! adds read-only battery / VBUS monitoring through reviewed register helpers.

use core::fmt::Debug;

use anyhow::{anyhow, bail, Result};
use embedded_hal::i2c::I2c;

use crate::power_key::{power_key_event_from_irq_status, PowerKeyEvent, POWER_KEY_EVENT_MASK};

const AXP2101_ADDRESS: u8 = 0x34;
const STATUS1: u8 = 0x00;
const STATUS2: u8 = 0x01;
const IC_TYPE: u8 = 0x03;
const AXP2101_CHIP_ID: u8 = 0x4A;
const ADC_CHANNEL_CTRL: u8 = 0x30;
const ADC_BAT_VOLTAGE_HIGH: u8 = 0x34;
const ADC_BAT_VOLTAGE_LOW: u8 = 0x35;
const BAT_DET_CTRL: u8 = 0x68;
const DC_ONOFF_DVM_CTRL: u8 = 0x80;
const LDO_ONOFF_CTRL0: u8 = 0x90;
const LDO_ONOFF_CTRL1: u8 = 0x91;
const LDO_VOL2_CTRL: u8 = 0x94;
const ALDO2_VOL_CTRL: u8 = 0x93;
const BAT_PERCENT_DATA: u8 = 0xA4;
const INTEN2: u8 = 0x41;
const INTSTS2: u8 = 0x49;
/// `STATUS2` bits `[2:0]`: charger state machine. 0-2 are the active
/// constant-current phases (trickle/pre-charge/constant-current), 3 is
/// constant-voltage (still actively charging), 4 is charge-done (full while
/// still on VBUS), and 5 is not-charging. Cross-checked against the Zephyr
/// `charger_axp2101.c` driver (`CHARGING_STATUS = GENMASK(2, 0)`) and
/// XPowersLib's `getChargerStatus()`, since only image-scanned datasheet
/// copies were available (see the [`COMMON_CONFIG`] doc for the same
/// caveat). The previous `status2 >> 5` read bits `[7:5]` instead, which are
/// undocumented for this purpose, so `charging` never reliably reflected the
/// PMIC's real state.
const CHG_STATUS_MASK: u8 = 0b0000_0111;
/// Highest `CHG_STATUS_MASK` value that still means "actively charging".
const CHG_STATUS_CONSTANT_VOLTAGE: u8 = 0x03;
/// PMU common configuration register. Bit 0 is the software power-off
/// trigger: writing 1 cuts every PMIC output immediately, the same as
/// holding the physical Power key past the hardware `OFFLEVEL` timeout.
/// Address and bit position cross-checked against the `axp2101-embedded`
/// crate's register map (`AXP2101_COMMON_CONFIG`) and XPowersLib's
/// `AXP2101Constants.h` (`XPOWERS_AXP2101_COMMON_CONFIG` /
/// `XPOWERS_AXP2101_SOFT_OFF_BIT`); bit-level behavior still needs
/// confirmation against a real AXP2101 datasheet copy during hardware
/// bring-up, since only image-scanned datasheet copies were available.
const COMMON_CONFIG: u8 = 0x10;
const SOFT_POWER_OFF_BIT: u8 = 1 << 0;
/// First of four general-purpose one-byte scratch registers
/// (`DATA_BUFFER1`-`DATA_BUFFER4`, 0x04-0x07) that XPowersLib documents as
/// surviving a software power-off while the battery stays connected. Only
/// one byte is needed to flag "this shutdown was requested by firmware, not
/// an external reset", so the other three (0x05-0x07) are left unused.
const DATA_BUFFER1: u8 = 0x04;
/// Arbitrary non-zero, non-0xFF marker so an unwritten/erased register
/// (which reads back as 0x00 after a real power-on-reset or a battery
/// disconnect) is never mistaken for a firmware-requested shutdown.
const PMIC_SHUTDOWN_MARKER: u8 = 0x5A;
/// Power-on source (read-only): which event most recently powered the PMIC
/// on. Logged at boot as corroborating evidence alongside the
/// [`DATA_BUFFER1`] marker, never as the sole signal.
const PWRON_STATUS: u8 = 0x20;
/// Power-off source (read-only): which event most recently powered the PMIC
/// off. Logged at boot for the same reason as [`PWRON_STATUS`].
const PWROFF_STATUS: u8 = 0x21;
/// Power-off enable / long-press power-off behavior control. Logged at boot;
/// not written by this driver, since the exact bit layout was not
/// confirmed against a real datasheet copy (see [`COMMON_CONFIG`] doc).
const PWROFF_EN: u8 = 0x22;
/// Combined IRQLEVEL (long-press threshold) / OFFLEVEL (hardware forced
/// power-off hold time) / ONLEVEL (power-key press time to power back on)
/// timing register. Logged at boot as a raw byte only: this driver does not
/// decode or rewrite individual sub-fields, since their exact bit
/// boundaries were not confirmed against a real datasheet copy and an
/// incorrect write here risks shortening `OFFLEVEL` below the long-press
/// threshold, which would make the maintenance menu also cut power.
const IRQ_OFF_ON_LEVEL_CTRL: u8 = 0x27;
const ALDO1_ENABLE_BIT: u8 = 1 << 0;
const ALDO2_ENABLE_BIT: u8 = 1 << 1;
const ALDO3_ENABLE_BIT: u8 = 1 << 2;
const ALDO3_MIN_MV: u16 = 500;
const ALDO3_MAX_MV: u16 = 3500;
const ALDO3_STEP_MV: u16 = 100;
const EPAPER_RAIL_MV: u16 = 3300;
/// ES8311 AVDD and the onboard digital microphone share this AXP2101 output.
const AUDIO_RAIL_MV: u16 = 3300;

// Schematic trace confirmed these AXP2101 outputs are unpopulated on this
// board revision: ALDO1 dead-ends on unmounted R74, ALDO4/BLDO1/BLDO2/
// CPUSLDO/DLDO1/DLDO2 have no net at all, and DCDC2-4 are switching channels
// with no inductor populated on LX. Register/bit values verified against
// XPowersLib's AXP2101Constants.h. DCDC1 (system VCC3V3, bit0 of
// `DC_ONOFF_DVM_CTRL`) and DCDC5 (bit4) are deliberately excluded from the
// mask below and are never touched.
const ALDO4_ENABLE_BIT: u8 = 1 << 3;
const BLDO1_ENABLE_BIT: u8 = 1 << 4;
const BLDO2_ENABLE_BIT: u8 = 1 << 5;
const CPUSLDO_ENABLE_BIT: u8 = 1 << 6;
const DLDO1_ENABLE_BIT: u8 = 1 << 7;
const UNUSED_LDO_ONOFF_CTRL0_BITS: u8 = ALDO1_ENABLE_BIT
    | ALDO4_ENABLE_BIT
    | BLDO1_ENABLE_BIT
    | BLDO2_ENABLE_BIT
    | CPUSLDO_ENABLE_BIT
    | DLDO1_ENABLE_BIT;
const DLDO2_ENABLE_BIT: u8 = 1 << 0;
const DCDC2_ENABLE_BIT: u8 = 1 << 1;
const DCDC3_ENABLE_BIT: u8 = 1 << 2;
const DCDC4_ENABLE_BIT: u8 = 1 << 3;
const UNUSED_DC_ONOFF_DVM_CTRL_BITS: u8 = DCDC2_ENABLE_BIT | DCDC3_ENABLE_BIT | DCDC4_ENABLE_BIT;

/// Read-only AXP2101 status consumed by the sample-app UI shell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerSnapshot {
    /// Fuel-gauge percentage when a battery is detected.
    pub battery_percent: Option<u8>,
    /// Battery voltage in millivolts when a battery is detected.
    pub battery_voltage_mv: Option<u16>,
    /// USB VBUS is present and reported good by the PMIC.
    pub vbus_present: bool,
    /// PMIC charge state reports active charging.
    pub charging: bool,
}

/// Largest single-sample change in [`PowerSnapshot::battery_percent`]
/// accepted immediately as ordinary drift. The AXP2101 fuel gauge has been
/// observed to report a implausible one-shot jump (tens of percentage
/// points) around load transients, USB plug/unplug bounce, or an internal
/// coulomb-counter recalibration; this stays well above normal drift between
/// two reads but well below those observed glitches.
const MAX_PLAUSIBLE_BATTERY_STEP: u8 = 8;

/// Suppresses a single implausible [`PowerSnapshot::battery_percent`]
/// glitch: a jump larger than [`MAX_PLAUSIBLE_BATTERY_STEP`] is held back
/// for one read and only accepted once a following read confirms it (agrees
/// with the held-back value within the same tolerance), so a lone bad
/// fuel-gauge sample never reaches the UI. A genuine change (e.g. the
/// battery actually drained a lot while the device sat unused) still shows
/// up, just one read later than the raw register.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatteryPercentFilter {
    accepted: Option<u8>,
    pending: Option<u8>,
}

impl BatteryPercentFilter {
    /// Feed one raw fuel-gauge reading and get back the filtered value to
    /// show. The very first reading (no prior state, e.g. at boot) is always
    /// trusted since there is nothing yet to compare it against.
    pub fn apply(&mut self, raw: u8) -> u8 {
        let next = match self.accepted {
            None => raw,
            Some(accepted) => {
                if raw.abs_diff(accepted) <= MAX_PLAUSIBLE_BATTERY_STEP {
                    self.pending = None;
                    raw
                } else if self
                    .pending
                    .is_some_and(|pending| pending.abs_diff(raw) <= MAX_PLAUSIBLE_BATTERY_STEP)
                {
                    self.pending = None;
                    raw
                } else {
                    self.pending = Some(raw);
                    accepted
                }
            }
        };
        self.accepted = Some(next);
        next
    }
}

/// Power contract consumed by the panel driver.
pub trait PanelPower {
    /// Configure and enable the panel rail.
    fn enable_panel_rail(&mut self) -> Result<()>;
    /// Disable the panel rail after e-paper deep sleep.
    fn disable_panel_rail(&mut self) -> Result<()>;
}

/// Narrow AXP2101 register-level driver.
pub struct Axp2101<I2C> {
    i2c: I2C,
}

impl<I2C> Axp2101<I2C> {
    #[must_use]
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }
}

impl<I2C> Axp2101<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    fn read_register(&mut self, register: u8) -> Result<u8> {
        let mut value = [0_u8; 1];
        self.i2c
            .write_read(AXP2101_ADDRESS, &[register], &mut value)
            .map_err(|error| anyhow!("AXP2101 read 0x{register:02X} failed: {error:?}"))?;
        Ok(value[0])
    }

    fn write_register(&mut self, register: u8, value: u8) -> Result<()> {
        self.i2c
            .write(AXP2101_ADDRESS, &[register, value])
            .map_err(|error| anyhow!("AXP2101 write 0x{register:02X} failed: {error:?}"))
    }

    fn update_bits(&mut self, register: u8, mask: u8, enabled: bool) -> Result<()> {
        let value = self.read_register(register)?;
        let next = if enabled { value | mask } else { value & !mask };
        self.write_register(register, next)
    }

    fn verify_present(&mut self) -> Result<()> {
        let chip_id = self.read_register(IC_TYPE)?;
        if chip_id != AXP2101_CHIP_ID {
            bail!(
                "unexpected PMIC chip ID: got 0x{chip_id:02X}, expected AXP2101 0x{AXP2101_CHIP_ID:02X}"
            );
        }
        Ok(())
    }

    fn set_aldo3_voltage_mv(&mut self, millivolts: u16) -> Result<()> {
        let current = self.read_register(LDO_VOL2_CTRL)?;
        let encoded = encode_aldo3_voltage_mv(millivolts)?;
        self.write_register(LDO_VOL2_CTRL, (current & 0xE0) | encoded)
    }

    /// ALDO2 uses the same 500-3500 mV / 100 mV-step encoding as ALDO3.
    fn set_aldo2_voltage_mv(&mut self, millivolts: u16) -> Result<()> {
        let current = self.read_register(ALDO2_VOL_CTRL)?;
        let encoded = encode_aldo3_voltage_mv(millivolts)?;
        self.write_register(ALDO2_VOL_CTRL, (current & 0xE0) | encoded)
    }

    /// Disable every AXP2101 output the schematic trace confirmed is
    /// unpopulated on this board revision (ALDO1, ALDO4, BLDO1, BLDO2,
    /// CPUSLDO, DLDO1, DLDO2, DCDC2-4) once at boot, regardless of whatever
    /// the PMIC's power-on default leaves enabled. DCDC1 (system VCC3V3) and
    /// DCDC5 are excluded by construction: the masks only ever clear the bits
    /// listed above, so any other channel's enable state is left untouched.
    pub fn disable_unused_pmic_rails(&mut self) -> Result<()> {
        self.verify_present()?;
        self.update_bits(LDO_ONOFF_CTRL0, UNUSED_LDO_ONOFF_CTRL0_BITS, false)?;
        self.update_bits(LDO_ONOFF_CTRL1, DLDO2_ENABLE_BIT, false)?;
        self.update_bits(DC_ONOFF_DVM_CTRL, UNUSED_DC_ONOFF_DVM_CTRL_BITS, false)
    }

    /// Enable ALDO2 (Audio_VCC), which feeds the ES8311 codec AVDD pin and the
    /// onboard digital microphone, before the audio subsystem initializes.
    pub fn enable_audio_rail(&mut self) -> Result<()> {
        self.verify_present()?;
        self.set_aldo2_voltage_mv(AUDIO_RAIL_MV)?;
        self.update_bits(LDO_ONOFF_CTRL0, ALDO2_ENABLE_BIT, true)
    }

    /// Disable ALDO2 before MCU deep sleep, mirroring [`PanelPower::disable_panel_rail`].
    /// PVDD/DVDD stay powered from the always-on VCC3V3 rail regardless.
    pub fn disable_audio_rail(&mut self) -> Result<()> {
        self.update_bits(LDO_ONOFF_CTRL0, ALDO2_ENABLE_BIT, false)
    }

    /// Enable only the PMIC measurement channels needed by the sample-app
    /// status slice. Charging policy remains untouched.
    pub fn initialize_sample_monitoring(&mut self) -> Result<()> {
        self.verify_present()?;
        self.update_bits(BAT_DET_CTRL, 1 << 0, true)?;
        self.update_bits(ADC_CHANNEL_CTRL, 1 << 0, true)
    }

    /// Enable AXP2101 short- and long-press Power-key reporting and clear any
    /// stale key status before the event loop starts.
    pub fn initialize_power_key_events(&mut self) -> Result<()> {
        self.verify_present()?;
        self.update_bits(INTEN2, POWER_KEY_EVENT_MASK, true)?;
        self.write_register(INTSTS2, POWER_KEY_EVENT_MASK)
    }

    /// Read and clear one latched AXP2101 Power-key event. Long press takes
    /// priority when both sticky bits are present.
    ///
    /// The status register is write-one-to-clear. Unrelated PMIC IRQ status bits
    /// are deliberately preserved for later isolated milestones.
    ///
    /// No `verify_present()` here: this runs on the 100ms power-key poll for
    /// the device's entire awake lifetime, and the chip identity can't change
    /// at runtime once `initialize_power_key_events` has confirmed it at boot.
    pub fn take_power_key_event(&mut self) -> Result<Option<PowerKeyEvent>> {
        let status2 = self.read_register(INTSTS2)?;
        let event = power_key_event_from_irq_status(status2);
        let latched_key_bits = status2 & POWER_KEY_EVENT_MASK;
        if latched_key_bits != 0 {
            self.write_register(INTSTS2, latched_key_bits)?;
        }
        Ok(event)
    }

    /// Cut power to every PMIC output immediately via the software power-off
    /// bit. Unlike MCU deep sleep, this call does not return on success: the
    /// PMIC drops the board's rails before the I2C transaction's own ACK can
    /// be observed by the caller in practice, so callers should treat any
    /// `Ok(())` return (should the bus somehow survive long enough to report
    /// one) the same as a timeout -- proceed to the deep-sleep fallback
    /// rather than assuming the device is still usable.
    pub fn power_off(&mut self) -> Result<()> {
        self.verify_present()?;
        self.update_bits(COMMON_CONFIG, SOFT_POWER_OFF_BIT, true)
    }

    /// Record "this shutdown was requested by firmware" in a PMIC scratch
    /// register that survives a software power-off while the battery stays
    /// connected, so the next boot can tell a PMIC power-key wake apart from
    /// a plain power-on/reset. Call only after every other pre-shutdown
    /// write (sleep-image marker, Reader persistence) has been flushed to
    /// SD, since [`power_off`](Self::power_off) cuts power with no further
    /// warning.
    pub fn write_shutdown_marker(&mut self) -> Result<()> {
        self.verify_present()?;
        self.write_register(DATA_BUFFER1, PMIC_SHUTDOWN_MARKER)
    }

    /// Read and immediately clear the PMIC shutdown marker. Must run once,
    /// early at boot, right after this driver confirms the chip is present
    /// and before anything else reads [`DATA_BUFFER1`]: clearing here
    /// guarantees a later reset or power-on that never went through
    /// [`power_off`](Self::power_off) cannot be mistaken for a PMIC wake.
    pub fn take_shutdown_marker(&mut self) -> Result<bool> {
        self.verify_present()?;
        let marker = self.read_register(DATA_BUFFER1)?;
        self.write_register(DATA_BUFFER1, 0x00)?;
        Ok(marker == PMIC_SHUTDOWN_MARKER)
    }

    /// Raw `(PWRON_STATUS, PWROFF_STATUS)` bytes for boot-time logging only.
    /// Corroborating evidence alongside [`take_shutdown_marker`]; this driver
    /// does not decode individual source bits since their layout was not
    /// confirmed against a real datasheet copy.
    pub fn read_power_on_off_source(&mut self) -> Result<(u8, u8)> {
        Ok((self.read_register(PWRON_STATUS)?, self.read_register(PWROFF_STATUS)?))
    }

    /// Raw `(PWROFF_EN, IRQ_OFF_ON_LEVEL_CTRL)` bytes for boot-time logging
    /// only -- see the doc comments on [`PWROFF_EN`] and
    /// [`IRQ_OFF_ON_LEVEL_CTRL`] for why this driver does not decode or
    /// rewrite the individual timing sub-fields yet.
    pub fn read_power_key_timing_config(&mut self) -> Result<(u8, u8)> {
        Ok((self.read_register(PWROFF_EN)?, self.read_register(IRQ_OFF_ON_LEVEL_CTRL)?))
    }

    /// Read battery and VBUS state using the same AXP2101 register meanings as
    /// the uploaded sample application.
    ///
    /// No `verify_present()` here: this runs on every board-status snapshot
    /// (button presses plus the periodic live-status refresh), and the chip
    /// identity can't change at runtime once boot-time init confirmed it.
    pub fn read_power_snapshot(&mut self) -> Result<PowerSnapshot> {
        let status1 = self.read_register(STATUS1)?;
        let status2 = self.read_register(STATUS2)?;
        let battery_connected = status1 & (1 << 3) != 0;

        let battery_percent = if battery_connected {
            Some(self.read_register(BAT_PERCENT_DATA)?.min(100))
        } else {
            None
        };
        let battery_voltage_mv = if battery_connected {
            let high = self.read_register(ADC_BAT_VOLTAGE_HIGH)?;
            let low = self.read_register(ADC_BAT_VOLTAGE_LOW)?;
            Some(decode_battery_voltage_mv(high, low))
        } else {
            None
        };

        Ok(PowerSnapshot {
            battery_percent,
            battery_voltage_mv,
            vbus_present: status1 & (1 << 5) != 0 && status2 & (1 << 3) == 0,
            charging: is_actively_charging(status2),
        })
    }
}

impl<I2C> PanelPower for Axp2101<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    fn enable_panel_rail(&mut self) -> Result<()> {
        self.verify_present()?;
        self.set_aldo3_voltage_mv(EPAPER_RAIL_MV)?;
        self.update_bits(LDO_ONOFF_CTRL0, ALDO3_ENABLE_BIT, true)
    }

    fn disable_panel_rail(&mut self) -> Result<()> {
        self.update_bits(LDO_ONOFF_CTRL0, ALDO3_ENABLE_BIT, false)
    }
}

fn decode_battery_voltage_mv(high: u8, low: u8) -> u16 {
    (u16::from(high & 0x1F) << 8) | u16::from(low)
}

/// True while the AXP2101 charger state machine (`STATUS2` bits `[2:0]`) is
/// in any of the active charging phases (trickle, pre-charge, constant
/// current, or constant voltage). False once the battery is full
/// (charge-done) or nothing is charging.
fn is_actively_charging(status2: u8) -> bool {
    status2 & CHG_STATUS_MASK <= CHG_STATUS_CONSTANT_VOLTAGE
}

fn encode_aldo3_voltage_mv(millivolts: u16) -> Result<u8> {
    if !(ALDO3_MIN_MV..=ALDO3_MAX_MV).contains(&millivolts) {
        bail!("ALDO3 voltage {millivolts} mV is outside the supported range");
    }
    if millivolts % ALDO3_STEP_MV != 0 {
        bail!("ALDO3 voltage {millivolts} mV must use {ALDO3_STEP_MV} mV steps");
    }

    Ok(((millivolts - ALDO3_MIN_MV) / ALDO3_STEP_MV) as u8)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_battery_voltage_mv, encode_aldo3_voltage_mv, is_actively_charging,
        BatteryPercentFilter, ALDO1_ENABLE_BIT, ALDO2_ENABLE_BIT, ALDO3_ENABLE_BIT,
        ALDO4_ENABLE_BIT, BLDO1_ENABLE_BIT, BLDO2_ENABLE_BIT, CPUSLDO_ENABLE_BIT,
        DCDC2_ENABLE_BIT, DCDC3_ENABLE_BIT, DCDC4_ENABLE_BIT, DLDO1_ENABLE_BIT, DLDO2_ENABLE_BIT,
        UNUSED_DC_ONOFF_DVM_CTRL_BITS, UNUSED_LDO_ONOFF_CTRL0_BITS, COMMON_CONFIG, DATA_BUFFER1,
        IRQ_OFF_ON_LEVEL_CTRL, PMIC_SHUTDOWN_MARKER, PWROFF_EN, PWROFF_STATUS, PWRON_STATUS,
        SOFT_POWER_OFF_BIT,
    };

    #[test]
    fn decodes_reference_battery_voltage_registers() {
        assert_eq!(decode_battery_voltage_mv(0x0F, 0x8C), 3_980);
    }

    #[test]
    fn charging_reads_status2_low_bits_not_high_bits() {
        // Trickle, pre-charge, constant-current, constant-voltage: charging.
        for status2 in [0x00, 0x01, 0x02, 0x03] {
            assert!(
                is_actively_charging(status2),
                "status2=0x{status2:02X} should read as charging"
            );
        }
        // Charge-done, not-charging: not charging.
        for status2 in [0x04, 0x05] {
            assert!(
                !is_actively_charging(status2),
                "status2=0x{status2:02X} should not read as charging"
            );
        }
        // A value that would have falsely reported "charging" under the old
        // `status2 >> 5 == 0x01` decode (bits [7:5] = 0b001) but whose real
        // charger-status bits [2:0] say "not charging".
        assert!(!is_actively_charging(0b0010_0101));
    }

    #[test]
    fn battery_percent_filter_trusts_the_first_ever_reading() {
        let mut filter = BatteryPercentFilter::default();
        assert_eq!(filter.apply(25), 25);
    }

    #[test]
    fn battery_percent_filter_accepts_ordinary_drift_immediately() {
        let mut filter = BatteryPercentFilter::default();
        filter.apply(50);
        assert_eq!(filter.apply(47), 47);
        assert_eq!(filter.apply(45), 45);
    }

    #[test]
    fn battery_percent_filter_suppresses_a_lone_glitch() {
        let mut filter = BatteryPercentFilter::default();
        assert_eq!(filter.apply(25), 25);
        // A single implausible 25 -> 50 jump: held back, not shown yet.
        assert_eq!(filter.apply(50), 25);
        // The very next read is back to normal: the 50 never repeats, so it
        // was correctly treated as a glitch and never reached the UI.
        assert_eq!(filter.apply(26), 26);
    }

    #[test]
    fn battery_percent_filter_accepts_a_confirmed_jump_one_read_late() {
        let mut filter = BatteryPercentFilter::default();
        assert_eq!(filter.apply(25), 25);
        // First 50 is held back pending confirmation.
        assert_eq!(filter.apply(50), 25);
        // A second consecutive reading near 50 confirms it as a real change.
        assert_eq!(filter.apply(50), 50);
        assert_eq!(filter.apply(49), 49);
    }

    #[test]
    fn aldo_enable_bits_are_distinct_and_match_the_xpowers_register_layout() {
        assert_eq!(ALDO1_ENABLE_BIT, 1 << 0);
        assert_eq!(ALDO2_ENABLE_BIT, 1 << 1);
        assert_eq!(ALDO3_ENABLE_BIT, 1 << 2);
        assert_eq!(ALDO4_ENABLE_BIT, 1 << 3);
        assert_eq!(BLDO1_ENABLE_BIT, 1 << 4);
        assert_eq!(BLDO2_ENABLE_BIT, 1 << 5);
        assert_eq!(CPUSLDO_ENABLE_BIT, 1 << 6);
        assert_eq!(DLDO1_ENABLE_BIT, 1 << 7);
        assert_eq!(DLDO2_ENABLE_BIT, 1 << 0);
    }

    #[test]
    fn unused_rail_masks_never_include_dcdc1_or_dcdc5() {
        assert_eq!(DCDC2_ENABLE_BIT, 1 << 1);
        assert_eq!(DCDC3_ENABLE_BIT, 1 << 2);
        assert_eq!(DCDC4_ENABLE_BIT, 1 << 3);
        assert_eq!(
            UNUSED_DC_ONOFF_DVM_CTRL_BITS & 1,
            0,
            "DCDC1 bit must stay untouched"
        );
        assert_eq!(
            UNUSED_DC_ONOFF_DVM_CTRL_BITS & (1 << 4),
            0,
            "DCDC5 bit must stay untouched"
        );
    }

    #[test]
    fn unused_ldo_ctrl0_mask_excludes_aldo2_and_aldo3() {
        assert_eq!(
            UNUSED_LDO_ONOFF_CTRL0_BITS,
            ALDO1_ENABLE_BIT
                | ALDO4_ENABLE_BIT
                | BLDO1_ENABLE_BIT
                | BLDO2_ENABLE_BIT
                | CPUSLDO_ENABLE_BIT
                | DLDO1_ENABLE_BIT
        );
        assert_eq!(UNUSED_LDO_ONOFF_CTRL0_BITS & ALDO2_ENABLE_BIT, 0);
        assert_eq!(UNUSED_LDO_ONOFF_CTRL0_BITS & ALDO3_ENABLE_BIT, 0);
    }

    #[test]
    fn encodes_epaper_rail_voltage() {
        assert_eq!(encode_aldo3_voltage_mv(3_300).unwrap(), 0x1C);
    }

    #[test]
    fn rejects_out_of_range_voltage() {
        assert!(encode_aldo3_voltage_mv(400).is_err());
        assert!(encode_aldo3_voltage_mv(3_600).is_err());
    }

    #[test]
    fn rejects_non_step_voltage() {
        assert!(encode_aldo3_voltage_mv(3_350).is_err());
    }

    #[test]
    fn power_off_register_and_bit_match_the_xpowers_register_layout() {
        assert_eq!(COMMON_CONFIG, 0x10);
        assert_eq!(SOFT_POWER_OFF_BIT, 0x01);
    }

    #[test]
    fn shutdown_marker_register_and_value_are_distinguishable_from_a_cleared_register() {
        assert_eq!(DATA_BUFFER1, 0x04);
        assert_ne!(
            PMIC_SHUTDOWN_MARKER, 0x00,
            "0x00 is what a real power-on-reset or battery disconnect leaves behind"
        );
        assert_ne!(PMIC_SHUTDOWN_MARKER, 0xFF);
    }

    #[test]
    fn power_source_and_timing_registers_match_the_xpowers_register_layout() {
        assert_eq!(PWRON_STATUS, 0x20);
        assert_eq!(PWROFF_STATUS, 0x21);
        assert_eq!(PWROFF_EN, 0x22);
        assert_eq!(IRQ_OFF_ON_LEVEL_CTRL, 0x27);
    }
}
