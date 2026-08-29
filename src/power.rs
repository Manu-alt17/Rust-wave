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
    pub fn take_power_key_event(&mut self) -> Result<Option<PowerKeyEvent>> {
        self.verify_present()?;
        let status2 = self.read_register(INTSTS2)?;
        let event = power_key_event_from_irq_status(status2);
        let latched_key_bits = status2 & POWER_KEY_EVENT_MASK;
        if latched_key_bits != 0 {
            self.write_register(INTSTS2, latched_key_bits)?;
        }
        Ok(event)
    }

    /// Read battery and VBUS state using the same AXP2101 register meanings as
    /// the uploaded sample application.
    pub fn read_power_snapshot(&mut self) -> Result<PowerSnapshot> {
        self.verify_present()?;
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
            charging: status2 >> 5 == 0x01,
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
        decode_battery_voltage_mv, encode_aldo3_voltage_mv, ALDO1_ENABLE_BIT, ALDO2_ENABLE_BIT,
        ALDO3_ENABLE_BIT, ALDO4_ENABLE_BIT, BLDO1_ENABLE_BIT, BLDO2_ENABLE_BIT, CPUSLDO_ENABLE_BIT,
        DCDC2_ENABLE_BIT, DCDC3_ENABLE_BIT, DCDC4_ENABLE_BIT, DLDO1_ENABLE_BIT, DLDO2_ENABLE_BIT,
        UNUSED_DC_ONOFF_DVM_CTRL_BITS, UNUSED_LDO_ONOFF_CTRL0_BITS,
    };

    #[test]
    fn decodes_reference_battery_voltage_registers() {
        assert_eq!(decode_battery_voltage_mv(0x0F, 0x8C), 3_980);
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
}
