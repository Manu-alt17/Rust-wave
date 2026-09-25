//! QMI8658 six-axis motion diagnostics ported from the uploaded sample app.
//!
//! The first IMU milestone intentionally keeps the protocol narrow: probe the
//! two documented SA0 addresses, verify `WHO_AM_I`, apply the sample app's
//! accelerometer and gyroscope profile, and read one contiguous diagnostic
//! frame. Wake-on-motion interrupts remain a later power-management slice.

use core::fmt::Debug;

use anyhow::{anyhow, bail, Result};
use embedded_hal::{delay::DelayNs, i2c::I2c};

use crate::regional::Locale;

/// QMI8658 SA0-low address used by the uploaded Waveshare sample.
pub const QMI8658_ADDRESS_LOW: u8 = 0x6A;
/// QMI8658 SA0-high fallback address supported by the uploaded sample.
pub const QMI8658_ADDRESS_HIGH: u8 = 0x6B;
/// Device identifier required by the sample driver's probe loop.
pub const QMI8658_WHO_AM_I_VALUE: u8 = 0x05;

const WHO_AM_I: u8 = 0x00;
const REVISION: u8 = 0x01;
const CTRL1: u8 = 0x02;
const CTRL2: u8 = 0x03;
const CTRL3: u8 = 0x04;
const CTRL5: u8 = 0x06;
const CTRL7: u8 = 0x08;
const CTRL8: u8 = 0x09;
const CTRL9: u8 = 0x0A;
const CAL1_L: u8 = 0x0B;
const CAL1_H: u8 = 0x0C;
const CAL2_L: u8 = 0x0D;
const CAL2_H: u8 = 0x0E;
const CAL3_L: u8 = 0x0F;
const CAL3_H: u8 = 0x10;
const CAL4_H: u8 = 0x12;
const STATUS0: u8 = 0x2E;
const STATUS_INT: u8 = 0x2D;
const STATUS1: u8 = 0x2F;
// TEMPERATURE_L (0x33) starts 5 bytes after STATUS0 (0x2E) -- see the
// `burst[5..19]` slice in `read_motion`, which reads from there through
// the end of the gyroscope registers in one transaction.
const TAP_STATUS: u8 = 0x59;

// Uploaded sample profile: accelerometer +/-8 g at 1000 Hz, gyroscope
// +/-512 dps at 1000 Hz, ACC + GYR enabled. The sample clears CTRL5 after
// configuration, so this focused port preserves that reviewed behavior.
const SAMPLE_CTRL1: u8 = 0x60;
const SAMPLE_CTRL2_ACC_8G_1000HZ: u8 = 0x23;
const SAMPLE_CTRL3_GYR_512DPS_1000HZ: u8 = 0x43;
const SAMPLE_CTRL5: u8 = 0x00;
const SAMPLE_CTRL7_ACC_GYR_ENABLE: u8 = 0x03;
// Reduced-power orientation-only profile used while a screen that doesn't
// need the gyroscope (Reader, most of the UI) is active. The QMI8658's only
// way into accelerometer low-power mode is selecting one of its dedicated
// low-power aODR codes in CTRL2 -- there is no separate mode-enable bit.
// 0xC-0xF select 128/21/11/3 Hz low-power ODR respectively; 21 Hz (0xD) is
// already far above what a human can physically rotate a handheld device,
// so a future tilt/auto-rotate feature loses nothing switching to it versus
// the 1000 Hz profile above. CTRL7 bit 0 is accelerometer-enable, bit 1 is
// gyroscope-enable (see `SAMPLE_CTRL7_ACC_GYR_ENABLE`'s both-bits value);
// leaving bit 1 clear here powers the gyroscope circuit down entirely.
const LOW_POWER_CTRL2_ACC_8G_21HZ: u8 = 0x2D;
const LOW_POWER_CTRL7_ACC_ONLY_ENABLE: u8 = 0x01;
const ACCEL_LSB_PER_G: i32 = 1 << 12;
const GYRO_LSB_PER_DPS: i32 = 64;

// Tap-detection engine (diagnostic phase). Register offsets, CTRL9 command
// codes and the Configure Tap two-phase CAL-register payload below are taken
// from QST's official QMI8658 register map as ported by the widely-used
// lewisxhe/SensorLib driver (github.com/lewisxhe/SensorLib), cross-checked
// against the vendored QMI8658A datasheet in that same repository. They are
// the same values regardless of the A/B/C chip revision noted on the module
// silkscreen -- only WHO_AM_I and available feature set differ by revision.
// Still, treat the *tuning* constants (peak/tap/double-tap windows and
// thresholds) as starting points only: this whole module exists to gather
// the data needed to tune them empirically, per the diagnostic-phase brief.
const CTRL8_TAP_DETECTION_ENABLE_BIT: u8 = 0x01;
const STATUS_INT_CTRL9_DONE_BIT: u8 = 0x80;
/// STATUS1 (0x2F) bit 1: latched "a new tap classification is ready in
/// TAP_STATUS" flag. This -- not comparing `TAP_STATUS`'s raw byte across
/// polls -- is the correct edge signal: two genuinely separate taps on the
/// same axis/polarity/kind (e.g. two single taps in the same corner) decode
/// to the *same* `TAP_STATUS` byte, so a raw-byte comparison misses the
/// second one. Confirmed against the reference driver's own `update()` loop,
/// which gates every `getTapStatus()` call on this bit rather than on the
/// decoded value changing.
const STATUS1_TAP_EVENT_BIT: u8 = 0x02;
const CTRL_CMD_ACK: u8 = 0x00;
const CTRL_CMD_CONFIGURE_TAP: u8 = 0x0C;
/// Iterations of a 1 ms delay to wait for a CTRL9 command handshake step.
const CTRL9_COMMAND_TIMEOUT_ITERATIONS: u32 = 50;
/// Fixed IIR filter coefficients (Q1.7 fixed point) used by the reference
/// tap-detection driver: alpha=0.0625, gamma=0.25. The reference driver does
/// not expose these for tuning, so neither does this port.
const TAP_FILTER_ALPHA_Q1_7: u8 = 8;
const TAP_FILTER_GAMMA_Q1_7: u8 = 32;

/// Starting-point tap-detection parameters -- see the module-level comment
/// above `CTRL8_TAP_DETECTION_ENABLE_BIT`. Exposed as named constants so
/// they're easy to find and change during empirical tuning.
pub const TAP_CONFIG_PEAK_WINDOW_SAMPLES: u8 = 30;
pub const TAP_CONFIG_TAP_WINDOW_SAMPLES: u16 = 100;
/// ~300 ms at the 1000 Hz accelerometer ODR `Qmi8658::initialize` applies,
/// matching the diagnostic-phase brief's double-tap window target.
pub const TAP_CONFIG_DOUBLE_TAP_WINDOW_SAMPLES: u16 = 300;
/// Units of 0.001 g^2 (the QMI8658's native tap-threshold unit).
pub const TAP_CONFIG_PEAK_THRESHOLD_MG2: u16 = 1_500;
pub const TAP_CONFIG_QUIET_THRESHOLD_MG2: u16 = 500;

/// Signed fixed-point axis values in tenths of the displayed unit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Axis3Tenths {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Axis3Tenths {
    /// Render compact signed X / Y / Z values for logs.
    #[must_use]
    pub fn compact_label(self) -> String {
        format!(
            "x={} y={} z={}",
            format_tenths(self.x),
            format_tenths(self.y),
            format_tenths(self.z)
        )
    }
}

/// Dominant board-axis hint derived from the accelerometer vector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DominantAxis {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
    #[default]
    Unknown,
}

impl DominantAxis {
    /// Stable product-facing label that avoids assuming enclosure orientation.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PositiveX => "+X dominant",
            Self::NegativeX => "-X dominant",
            Self::PositiveY => "+Y dominant",
            Self::NegativeY => "-Y dominant",
            Self::PositiveZ => "+Z dominant",
            Self::NegativeZ => "-Z dominant",
            Self::Unknown => "unknown",
        }
    }

    /// Locale-aware sibling of [`Self::label`] for on-screen display.
    /// `label` itself is left untouched because `src/main.rs`'s serial
    /// diagnostics logging depends on its English output staying stable.
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match locale {
            Locale::English => self.label(),
            Locale::Italian => match self {
                Self::PositiveX => "+X dominante",
                Self::NegativeX => "-X dominante",
                Self::PositiveY => "+Y dominante",
                Self::NegativeY => "-Y dominante",
                Self::PositiveZ => "+Z dominante",
                Self::NegativeZ => "-Z dominante",
                Self::Unknown => "sconosciuto",
            },
        }
    }
}

/// One hardware-independent QMI8658 diagnostic snapshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImuReading {
    /// Acceleration in tenths of a milligravity unit.
    pub acceleration_mg_tenths: Axis3Tenths,
    /// Angular velocity in tenths of a degree per second.
    pub gyroscope_dps_tenths: Axis3Tenths,
    /// QMI8658 die temperature in tenths of a degree Celsius.
    pub temperature_tenths_c: i16,
    /// Magnitude of the acceleration vector in whole milligravity units.
    pub motion_magnitude_mg: u32,
    /// Dominant raw board axis derived from acceleration.
    pub dominant_axis: DominantAxis,
    /// QMI8658 STATUS0 value captured beside the sample.
    pub status0: u8,
}

impl ImuReading {
    /// Header-friendly motion magnitude label.
    #[must_use]
    pub fn magnitude_label(self) -> String {
        format!("{} mg", self.motion_magnitude_mg)
    }

    /// QMI8658 die-temperature label for diagnostics.
    #[must_use]
    pub fn temperature_label(self) -> String {
        format!("{} C", format_tenths(i32::from(self.temperature_tenths_c)))
    }
}

/// Successful QMI8658 startup report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImuInitReport {
    pub address: u8,
    pub who_am_i: u8,
    pub revision: u8,
}

/// Axis priority ordering the tap engine uses to classify which axis a
/// detected peak belongs to when more than one axis crosses the peak
/// threshold together. `ZGtYGtX` favors the axis perpendicular to the
/// e-paper face, the natural default for a tap on the screen glass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TapPriority {
    XGtYGtZ,
    XGtZGtY,
    YGtXGtZ,
    YGtZGtX,
    ZGtXGtY,
    #[default]
    ZGtYGtX,
}

impl TapPriority {
    const fn register_value(self) -> u8 {
        match self {
            Self::XGtYGtZ => 0x00,
            Self::XGtZGtY => 0x01,
            Self::YGtXGtZ => 0x02,
            Self::YGtZGtX => 0x03,
            Self::ZGtXGtY => 0x04,
            Self::ZGtYGtX => 0x05,
        }
    }
}

/// `CTRL_CMD_CONFIGURE_TAP` parameters. See the module-level comment above
/// `CTRL8_TAP_DETECTION_ENABLE_BIT` for provenance; `Default` returns the
/// `TAP_CONFIG_*` starting-point constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TapConfig {
    pub priority: TapPriority,
    /// Duration (in accelerometer samples) of the peak the engine looks for.
    pub peak_window_samples: u8,
    /// Window (in accelerometer samples) after a peak in which a second
    /// peak still counts toward the same tap.
    pub tap_window_samples: u16,
    /// Window (in accelerometer samples) after a single tap in which a
    /// second tap is classified as a double tap instead of a new single tap.
    pub double_tap_window_samples: u16,
    /// Minimum peak magnitude, in units of 0.001 g^2, to register as a tap.
    pub peak_threshold_mg2: u16,
    /// Maximum magnitude, in units of 0.001 g^2, the signal must settle
    /// below between the peak and quiet phases of the tap.
    pub quiet_threshold_mg2: u16,
}

impl Default for TapConfig {
    fn default() -> Self {
        Self {
            priority: TapPriority::default(),
            peak_window_samples: TAP_CONFIG_PEAK_WINDOW_SAMPLES,
            tap_window_samples: TAP_CONFIG_TAP_WINDOW_SAMPLES,
            double_tap_window_samples: TAP_CONFIG_DOUBLE_TAP_WINDOW_SAMPLES,
            peak_threshold_mg2: TAP_CONFIG_PEAK_THRESHOLD_MG2,
            quiet_threshold_mg2: TAP_CONFIG_QUIET_THRESHOLD_MG2,
        }
    }
}

/// Board axis `TAP_STATUS.TAP_AXIS` reported the detected peak on. This is
/// the sensor's raw axis, not a screen-plane coordinate -- see this module's
/// header comment on why a tap position still needs the raw-sample burst.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapAxis {
    X,
    Y,
    Z,
    /// The two-bit `TAP_AXIS` field's fourth encoding; not documented as a
    /// valid output by the reference driver, kept so decoding never panics.
    Reserved,
}

impl TapAxis {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
            Self::Reserved => "?",
        }
    }
}

/// `TAP_STATUS.TAP_POLARITY` direction of the detected peak along its axis.
/// Bit meaning (0=positive/1=negative) follows the common QST convention
/// seen in other QMI8658 status/event register documentation -- verify
/// against the datasheet revision on hand before relying on the sign.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapPolarity {
    Positive,
    Negative,
}

impl TapPolarity {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Positive => "+",
            Self::Negative => "-",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapKind {
    Single,
    Double,
}

impl TapKind {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Double => "double",
        }
    }
}

/// Decoded `TAP_STATUS` (0x59) register. `kind` is `None` when `TAP_NUM`
/// reads 0 (no tap latched).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TapStatus {
    pub kind: Option<TapKind>,
    pub axis: TapAxis,
    pub polarity: TapPolarity,
    /// Raw register byte, kept so callers can detect repeated identical
    /// reads (this module does not know whether TAP_STATUS self-clears on
    /// read on every chip revision).
    pub raw: u8,
}

/// Narrow register-level QMI8658 driver.
pub struct Qmi8658<I2C> {
    i2c: I2C,
    address: Option<u8>,
}

impl<I2C> Qmi8658<I2C> {
    #[must_use]
    pub fn new(i2c: I2C) -> Self {
        Self { i2c, address: None }
    }
}

impl<I2C> Qmi8658<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    /// Probe both SA0 addresses, require the reference chip ID, and apply the
    /// uploaded sample app's accelerometer and gyroscope profile.
    pub fn initialize(&mut self) -> Result<ImuInitReport> {
        let address = self.probe_address()?;
        self.address = Some(address);
        let revision = self.read_register(address, REVISION)?;

        self.write_register(address, CTRL1, SAMPLE_CTRL1)?;
        self.write_register(address, CTRL2, SAMPLE_CTRL2_ACC_8G_1000HZ)?;
        self.write_register(address, CTRL3, SAMPLE_CTRL3_GYR_512DPS_1000HZ)?;
        self.write_register(address, CTRL5, SAMPLE_CTRL5)?;
        self.write_register(address, CTRL7, SAMPLE_CTRL7_ACC_GYR_ENABLE)?;

        let enabled = self.read_register(address, CTRL7)?;
        if enabled & SAMPLE_CTRL7_ACC_GYR_ENABLE != SAMPLE_CTRL7_ACC_GYR_ENABLE {
            bail!("QMI8658 CTRL7 verification failed: read 0x{enabled:02X}");
        }

        Ok(ImuInitReport {
            address,
            who_am_i: QMI8658_WHO_AM_I_VALUE,
            revision,
        })
    }

    /// Disable the accelerometer and gyroscope to cut QMI8658 current draw.
    ///
    /// The QMI8658 sits on the board's always-on VCC3V3 rail (it is not
    /// switched by an AXP2101 ALDO/BLDO channel like the e-paper panel), so
    /// this CTRL7 write is the only lever available to reduce its power use
    /// while the rest of the board is in MCU deep sleep.
    pub fn sleep(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 sleep requested before initialization"))?;
        self.write_register(address, CTRL7, 0x00)
    }

    /// Re-enable the accelerometer/gyroscope profile applied by
    /// [`Self::initialize`] after [`Self::sleep`].
    pub fn wake(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 wake requested before initialization"))?;
        self.write_register(address, CTRL7, SAMPLE_CTRL7_ACC_GYR_ENABLE)
    }

    /// Drop to the accelerometer-only, 21 Hz low-power profile while a
    /// screen that doesn't need the gyroscope is active (Reader today).
    /// Keeps live tilt/orientation data available -- unlike [`Self::sleep`],
    /// which stops sampling entirely -- for a future auto-rotate feature,
    /// while cutting the current an always-on 1000 Hz accelerometer+gyroscope
    /// stream draws for no consumer. Disabling both channels before
    /// reprogramming CTRL2 mirrors the config-then-enable order
    /// [`Self::initialize`] already uses. Reverse with [`Self::wake_full_rate`].
    pub fn enter_low_power_orientation_mode(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 low-power mode requested before initialization"))?;
        self.write_register(address, CTRL7, 0x00)?;
        self.write_register(address, CTRL2, LOW_POWER_CTRL2_ACC_8G_21HZ)?;
        self.write_register(address, CTRL7, LOW_POWER_CTRL7_ACC_ONLY_ENABLE)
    }

    /// Restore the full 1000 Hz accelerometer+gyroscope profile applied by
    /// [`Self::initialize`], after [`Self::enter_low_power_orientation_mode`].
    /// Motion Events and gyroscope-driven Lua games need this before they
    /// become the active screen.
    pub fn wake_full_rate(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 full-rate wake requested before initialization"))?;
        self.write_register(address, CTRL7, 0x00)?;
        self.write_register(address, CTRL2, SAMPLE_CTRL2_ACC_8G_1000HZ)?;
        self.write_register(address, CTRL7, SAMPLE_CTRL7_ACC_GYR_ENABLE)
    }

    /// Stage `config` into the CAL1-CAL4 registers and issue the two-phase
    /// `CTRL_CMD_CONFIGURE_TAP` handshake documented for the tap-detection
    /// engine (see this module's header comment for the reference source).
    ///
    /// The CAL1-CAL4 registers double as the accelerometer/gyroscope
    /// calibration-offset registers while those channels are enabled, so
    /// both are disabled for the duration of this call and restored to the
    /// [`Self::initialize`] profile afterward -- the same disable-then-
    /// restore shape [`Self::enter_low_power_orientation_mode`] already uses
    /// around a CTRL2 reprogram.
    pub fn configure_tap_detection<D: DelayNs>(
        &mut self,
        config: TapConfig,
        delay: &mut D,
    ) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 tap configuration requested before initialization"))?;

        self.write_register(address, CTRL7, 0x00)?;

        self.write_register(address, CAL1_L, config.peak_window_samples)?;
        self.write_register(address, CAL1_H, config.priority.register_value())?;
        self.write_register(address, CAL2_L, (config.tap_window_samples & 0xFF) as u8)?;
        self.write_register(address, CAL2_H, (config.tap_window_samples >> 8) as u8)?;
        self.write_register(
            address,
            CAL3_L,
            (config.double_tap_window_samples & 0xFF) as u8,
        )?;
        self.write_register(
            address,
            CAL3_H,
            (config.double_tap_window_samples >> 8) as u8,
        )?;
        self.write_register(address, CAL4_H, 0x01)?;
        self.write_command(address, CTRL_CMD_CONFIGURE_TAP, delay)?;

        self.write_register(address, CAL1_L, TAP_FILTER_ALPHA_Q1_7)?;
        self.write_register(address, CAL1_H, TAP_FILTER_GAMMA_Q1_7)?;
        self.write_register(address, CAL2_L, (config.peak_threshold_mg2 & 0xFF) as u8)?;
        self.write_register(address, CAL2_H, (config.peak_threshold_mg2 >> 8) as u8)?;
        self.write_register(address, CAL3_L, (config.quiet_threshold_mg2 & 0xFF) as u8)?;
        self.write_register(address, CAL3_H, (config.quiet_threshold_mg2 >> 8) as u8)?;
        self.write_register(address, CAL4_H, 0x02)?;
        self.write_command(address, CTRL_CMD_CONFIGURE_TAP, delay)?;

        self.write_register(address, CTRL7, SAMPLE_CTRL7_ACC_GYR_ENABLE)
    }

    /// Set `CTRL8.Tap_EN`. Only meaningful after
    /// [`Self::configure_tap_detection`] has staged tap parameters.
    pub fn enable_tap_detection(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 tap enable requested before initialization"))?;
        let ctrl8 = self.read_register(address, CTRL8)?;
        self.write_register(address, CTRL8, ctrl8 | CTRL8_TAP_DETECTION_ENABLE_BIT)
    }

    /// Clear `CTRL8.Tap_EN`.
    pub fn disable_tap_detection(&mut self) -> Result<()> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 tap disable requested before initialization"))?;
        let ctrl8 = self.read_register(address, CTRL8)?;
        self.write_register(address, CTRL8, ctrl8 & !CTRL8_TAP_DETECTION_ENABLE_BIT)
    }

    /// Read and decode `TAP_STATUS` (0x59) unconditionally, with no framing
    /// around whether it represents a *new* tap. Kept for direct register
    /// inspection; [`Self::poll_tap_event`] is what diagnostic polling
    /// should call.
    pub fn read_tap_status(&mut self) -> Result<TapStatus> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 tap status requested before initialization"))?;
        let raw = self.read_register(address, TAP_STATUS)?;
        Ok(decode_tap_status(raw))
    }

    /// Poll for a *new* tap event: checks `STATUS1.TAP_EVENT` first and only
    /// reads/decodes `TAP_STATUS` when it's set, returning `None` otherwise.
    /// This is the edge-correct alternative to comparing [`Self::read_tap_status`]
    /// results across polls -- see [`STATUS1_TAP_EVENT_BIT`]'s doc comment for
    /// why that comparison misses repeated identical taps. Callers poll this
    /// directly over I2C rather than through INT1/INT2 -- hardware tap
    /// interrupts remain the same later power-management slice noted for
    /// wake-on-motion at this module's top.
    pub fn poll_tap_event(&mut self) -> Result<Option<TapStatus>> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 tap poll requested before initialization"))?;
        let status1 = self.read_register(address, STATUS1)?;
        if status1 & STATUS1_TAP_EVENT_BIT == 0 {
            return Ok(None);
        }
        let raw = self.read_register(address, TAP_STATUS)?;
        Ok(Some(decode_tap_status(raw)))
    }

    /// CTRL9 command handshake: write the command, wait for
    /// `STATUS_INT.CmdDone`, write `CTRL_CMD_ACK`, then wait for `CmdDone`
    /// to clear. Bounded by [`CTRL9_COMMAND_TIMEOUT_ITERATIONS`] 1 ms steps
    /// at each stage so a wedged handshake surfaces as an error rather than
    /// hanging the caller.
    fn write_command<D: DelayNs>(&mut self, address: u8, command: u8, delay: &mut D) -> Result<()> {
        self.write_register(address, CTRL9, command)?;
        let mut done = false;
        for _ in 0..CTRL9_COMMAND_TIMEOUT_ITERATIONS {
            if self.read_register(address, STATUS_INT)? & STATUS_INT_CTRL9_DONE_BIT != 0 {
                done = true;
                break;
            }
            delay.delay_ms(1);
        }
        if !done {
            bail!(
                "QMI8658 CTRL9 command 0x{command:02X} timed out waiting for STATUS_INT completion bit"
            );
        }

        self.write_register(address, CTRL9, CTRL_CMD_ACK)?;
        for _ in 0..CTRL9_COMMAND_TIMEOUT_ITERATIONS {
            if self.read_register(address, STATUS_INT)? & STATUS_INT_CTRL9_DONE_BIT == 0 {
                return Ok(());
            }
            delay.delay_ms(1);
        }
        bail!("QMI8658 CTRL9 command 0x{command:02X} ack did not clear STATUS_INT completion bit")
    }

    /// Read STATUS0 through the temperature + accelerometer + gyroscope
    /// frame in one contiguous burst (STATUS1 and the timestamp registers
    /// that sit between them are read along for free and discarded). One
    /// I2C transaction instead of two also means status0 and the sample
    /// data can no longer be read a beat apart if a new sample lands
    /// mid-poll.
    pub fn read_motion(&mut self) -> Result<ImuReading> {
        let address = self
            .address
            .ok_or_else(|| anyhow!("QMI8658 read requested before initialization"))?;
        let mut burst = [0_u8; 19];
        self.i2c
            .write_read(address, &[STATUS0], &mut burst)
            .map_err(|error| anyhow!("QMI8658 motion frame read failed: {error:?}"))?;
        let status0 = burst[0];
        let mut frame = [0_u8; 14];
        frame.copy_from_slice(&burst[5..19]);
        Ok(decode_motion_frame(frame, status0))
    }

    fn probe_address(&mut self) -> Result<u8> {
        for address in [QMI8658_ADDRESS_LOW, QMI8658_ADDRESS_HIGH] {
            let mut who_am_i = [0_u8; 1];
            if self
                .i2c
                .write_read(address, &[WHO_AM_I], &mut who_am_i)
                .is_ok()
                && who_am_i[0] == QMI8658_WHO_AM_I_VALUE
            {
                return Ok(address);
            }
        }
        bail!(
            "QMI8658 probe failed: expected WHO_AM_I 0x{QMI8658_WHO_AM_I_VALUE:02X} at 0x{QMI8658_ADDRESS_LOW:02X} or 0x{QMI8658_ADDRESS_HIGH:02X}"
        )
    }

    fn read_register(&mut self, address: u8, register: u8) -> Result<u8> {
        let mut value = [0_u8; 1];
        self.i2c
            .write_read(address, &[register], &mut value)
            .map_err(|error| anyhow!("QMI8658 read 0x{register:02X} failed: {error:?}"))?;
        Ok(value[0])
    }

    fn write_register(&mut self, address: u8, register: u8, value: u8) -> Result<()> {
        self.i2c
            .write(address, &[register, value])
            .map_err(|error| anyhow!("QMI8658 write 0x{register:02X} failed: {error:?}"))
    }
}

fn decode_motion_frame(frame: [u8; 14], status0: u8) -> ImuReading {
    let temperature_raw = i16::from_le_bytes([frame[0], frame[1]]);
    let acceleration_raw = [
        i16::from_le_bytes([frame[2], frame[3]]),
        i16::from_le_bytes([frame[4], frame[5]]),
        i16::from_le_bytes([frame[6], frame[7]]),
    ];
    let gyroscope_raw = [
        i16::from_le_bytes([frame[8], frame[9]]),
        i16::from_le_bytes([frame[10], frame[11]]),
        i16::from_le_bytes([frame[12], frame[13]]),
    ];

    let acceleration_mg_tenths = Axis3Tenths {
        x: i32::from(acceleration_raw[0]) * 10_000 / ACCEL_LSB_PER_G,
        y: i32::from(acceleration_raw[1]) * 10_000 / ACCEL_LSB_PER_G,
        z: i32::from(acceleration_raw[2]) * 10_000 / ACCEL_LSB_PER_G,
    };
    let gyroscope_dps_tenths = Axis3Tenths {
        x: i32::from(gyroscope_raw[0]) * 10 / GYRO_LSB_PER_DPS,
        y: i32::from(gyroscope_raw[1]) * 10 / GYRO_LSB_PER_DPS,
        z: i32::from(gyroscope_raw[2]) * 10 / GYRO_LSB_PER_DPS,
    };

    ImuReading {
        acceleration_mg_tenths,
        gyroscope_dps_tenths,
        temperature_tenths_c: (i32::from(temperature_raw) * 10 / 256) as i16,
        motion_magnitude_mg: motion_magnitude_mg(acceleration_mg_tenths),
        dominant_axis: dominant_axis(acceleration_mg_tenths),
        status0,
    }
}

/// Decode `TAP_STATUS`: `TAP_NUM` in bits 1:0, `TAP_AXIS` in bits 5:4,
/// `TAP_POLARITY` in bit 7 -- see [`TapStatus`] for field provenance.
fn decode_tap_status(raw: u8) -> TapStatus {
    let kind = match raw & 0x03 {
        0x01 => Some(TapKind::Single),
        0x02 => Some(TapKind::Double),
        _ => None,
    };
    let axis = match (raw >> 4) & 0x03 {
        0x00 => TapAxis::X,
        0x01 => TapAxis::Y,
        0x02 => TapAxis::Z,
        _ => TapAxis::Reserved,
    };
    let polarity = if raw & 0x80 == 0 {
        TapPolarity::Positive
    } else {
        TapPolarity::Negative
    };
    TapStatus {
        kind,
        axis,
        polarity,
        raw,
    }
}

fn motion_magnitude_mg(acceleration: Axis3Tenths) -> u32 {
    let x = i64::from(acceleration.x) / 10;
    let y = i64::from(acceleration.y) / 10;
    let z = i64::from(acceleration.z) / 10;
    integer_sqrt((x * x + y * y + z * z) as u64) as u32
}

fn dominant_axis(acceleration: Axis3Tenths) -> DominantAxis {
    let values = [acceleration.x, acceleration.y, acceleration.z];
    let mut index = 0_usize;
    let mut value = values[0];
    for (candidate_index, candidate) in values.iter().copied().enumerate().skip(1) {
        if candidate.abs() > value.abs() {
            index = candidate_index;
            value = candidate;
        }
    }
    if value == 0 {
        return DominantAxis::Unknown;
    }
    match (index, value.is_positive()) {
        (0, true) => DominantAxis::PositiveX,
        (0, false) => DominantAxis::NegativeX,
        (1, true) => DominantAxis::PositiveY,
        (1, false) => DominantAxis::NegativeY,
        (2, true) => DominantAxis::PositiveZ,
        (2, false) => DominantAxis::NegativeZ,
        _ => DominantAxis::Unknown,
    }
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut next = (x + value / x) / 2;
    while next < x {
        x = next;
        next = (x + value / x) / 2;
    }
    x
}

/// Render a signed fixed-point tenths value without floating point.
#[must_use]
pub fn format_tenths(value: i32) -> String {
    let magnitude = i64::from(value).abs();
    let sign = if value < 0 { "-" } else { "" };
    format!("{sign}{}.{:01}", magnitude / 10, magnitude % 10)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_motion_frame, decode_tap_status, dominant_axis, format_tenths, integer_sqrt,
        Axis3Tenths, DominantAxis, TapAxis, TapKind, TapPolarity, TapPriority,
        CTRL8_TAP_DETECTION_ENABLE_BIT, CTRL_CMD_CONFIGURE_TAP, LOW_POWER_CTRL2_ACC_8G_21HZ,
        LOW_POWER_CTRL7_ACC_ONLY_ENABLE, SAMPLE_CTRL2_ACC_8G_1000HZ, SAMPLE_CTRL7_ACC_GYR_ENABLE,
        STATUS1_TAP_EVENT_BIT, STATUS_INT_CTRL9_DONE_BIT,
    };

    #[test]
    fn low_power_orientation_registers_match_qmi8658_low_power_odr_encoding() {
        // CTRL2 = aST(0) | range[6:4] (kept identical to the full-rate
        // profile's +/-8g range) | aODR[3:0]=0xD, the QMI8658's dedicated
        // low-power 21 Hz accelerometer code (0xC-0xF select 128/21/11/3 Hz
        // low-power ODR; there is no separate low-power mode-enable bit).
        assert_eq!(LOW_POWER_CTRL2_ACC_8G_21HZ, 0b0010_1101);
        assert_eq!(LOW_POWER_CTRL2_ACC_8G_21HZ & 0x0F, 0x0D);
        assert_eq!(
            LOW_POWER_CTRL2_ACC_8G_21HZ & 0x70,
            SAMPLE_CTRL2_ACC_8G_1000HZ & 0x70,
            "low-power profile must keep the same +/-8g range as the full-rate profile"
        );
        // CTRL7: accelerometer-enable bit set, gyroscope-enable bit clear.
        assert_eq!(LOW_POWER_CTRL7_ACC_ONLY_ENABLE, 0x01);
        assert_eq!(
            LOW_POWER_CTRL7_ACC_ONLY_ENABLE & SAMPLE_CTRL7_ACC_GYR_ENABLE,
            LOW_POWER_CTRL7_ACC_ONLY_ENABLE,
            "low-power CTRL7 must only set bits also set by the full accel+gyro profile"
        );
    }

    #[test]
    fn decodes_reference_profile_frame_without_floating_point() {
        let reading = decode_motion_frame(
            [
                0x00, 0x0A, // 10.0 C die temperature
                0x00, 0x10, // +4096 => +1000.0 mg X
                0x00, 0x00, // 0 mg Y
                0x00, 0x00, // 0 mg Z
                0x40, 0x00, // +64 => +1.0 dps X
                0x00, 0x00, // 0 dps Y
                0x00, 0x00, // 0 dps Z
            ],
            0x03,
        );
        assert_eq!(reading.temperature_tenths_c, 100);
        assert_eq!(reading.acceleration_mg_tenths.x, 10_000);
        assert_eq!(reading.gyroscope_dps_tenths.x, 10);
        assert_eq!(reading.motion_magnitude_mg, 1_000);
        assert_eq!(reading.dominant_axis, DominantAxis::PositiveX);
        assert_eq!(reading.status0, 0x03);
    }

    #[test]
    fn labels_signed_tenths() {
        assert_eq!(format_tenths(123), "12.3");
        assert_eq!(format_tenths(-7), "-0.7");
    }

    #[test]
    fn integer_sqrt_is_stable_for_motion_vectors() {
        assert_eq!(integer_sqrt(0), 0);
        assert_eq!(integer_sqrt(1_000_000), 1_000);
        assert_eq!(integer_sqrt(2_000_000), 1_414);
    }

    #[test]
    fn tap_engine_constants_match_qmi8658_register_map() {
        // CTRL8 bit 0 is Tap_EN; see this module's header comment for the
        // register-map source these values are ported from.
        assert_eq!(CTRL8_TAP_DETECTION_ENABLE_BIT, 0x01);
        // STATUS_INT bit 7 is the CTRL9 command-done handshake flag.
        assert_eq!(STATUS_INT_CTRL9_DONE_BIT, 0x80);
        assert_eq!(CTRL_CMD_CONFIGURE_TAP, 0x0C);
        assert_eq!(TapPriority::ZGtYGtX.register_value(), 0x05);
        assert_eq!(TapPriority::default(), TapPriority::ZGtYGtX);
        // STATUS1 bit 1 is the TAP_EVENT edge flag -- see this constant's
        // doc comment for why it, not a raw-byte comparison, is required.
        assert_eq!(STATUS1_TAP_EVENT_BIT, 0x02);
    }

    #[test]
    fn decodes_tap_status_single_positive_x() {
        // TAP_NUM=01 (single), TAP_AXIS=00 (X), TAP_POLARITY=0 (positive).
        let status = decode_tap_status(0b0_00_00_01);
        assert_eq!(status.kind, Some(TapKind::Single));
        assert_eq!(status.axis, TapAxis::X);
        assert_eq!(status.polarity, TapPolarity::Positive);
        assert_eq!(status.raw, 0x01);
    }

    #[test]
    fn decodes_tap_status_double_negative_z() {
        // TAP_POLARITY=1 (negative), TAP_AXIS=10 (Z, bits 5:4), TAP_NUM=10 (double, bits 1:0).
        let status = decode_tap_status(0b1_0_10_00_10);
        assert_eq!(status.kind, Some(TapKind::Double));
        assert_eq!(status.axis, TapAxis::Z);
        assert_eq!(status.polarity, TapPolarity::Negative);
    }

    #[test]
    fn decodes_tap_status_none_when_tap_num_zero() {
        let status = decode_tap_status(0b1_00_10_00);
        assert_eq!(status.kind, None);
    }

    #[test]
    fn dominant_axis_uses_largest_absolute_acceleration() {
        assert_eq!(
            dominant_axis(Axis3Tenths {
                x: 15,
                y: -999,
                z: 200,
            }),
            DominantAxis::NegativeY
        );
    }
}
