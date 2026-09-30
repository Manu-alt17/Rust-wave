//! Waveshare 3.97-inch 800 × 480 monochrome e-paper driver.
//!
//! The command sequence and refresh policy are ported from the uploaded
//! Waveshare ESP-IDF reference. The driver intentionally has no app/UI logic.

use core::fmt::Debug;

use anyhow::{anyhow, bail, Result};
use embedded_hal::{
    delay::DelayNs,
    digital::{InputPin, OutputPin},
    spi::SpiBus,
};
use log::{debug, info};

use crate::{
    framebuffer::{FRAMEBUFFER_SIZE, HEIGHT, WIDTH},
    power::PanelPower,
};

const BUSY_POLL_MS: u32 = 10;
const BUSY_TIMEOUT_MS: u32 = 15_000;
/// Settle time before the first BUSY sample. The SSD1677 raises BUSY within
/// microseconds of a command that starts internal work, so one short wait is
/// enough before polling; the pin itself then decides how long to wait. The
/// vendor reference used a fixed 100 ms here, which added ~90 ms of dead
/// time to every wait that finishes early (hardware reset, SWRESET and the
/// final init check -- up to ~270 ms on every panel wake-up).
const BUSY_SETTLE_MS: u32 = 10;

/// Value written to 0x1A before the fast global waveform. Despite the
/// register's "temperature" name this isn't a Celsius reading: it selects
/// one of the SSD1677 OTP's pre-baked LUT buckets, and each bucket is tied
/// to a specific waveform. 0x6A is the vendor-reference constant for the
/// monochrome "fast 1.5s" waveform used with 0x22 0xD7 (see Waveshare's
/// `EPD_Init_Fast` for this panel). 0x5A is a different bucket entirely —
/// the one for 4-gray mode — and loading it for a 1-bit frame is what
/// produced the inverted black/white output seen on real hardware.
const FAST_GLOBAL_TEMPERATURE: u8 = 0x6A;

/// Wait after releasing EPD_RST before polling BUSY, from the vendor
/// reference's reset sequence.
pub const RESET_RECOVERY_MS: u32 = 50;

/// Controller driver with explicit ownership of the panel bus and pins.
pub struct Epaper397<SPI, DC, RST, CS, BUSY, DELAY, POWER> {
    spi: SPI,
    dc: DC,
    reset: RST,
    cs: CS,
    busy: BUSY,
    delay: DELAY,
    power: POWER,
    /// Whether the controller's temperature register has been set since its
    /// last reset. Partial refreshes skip reading the sensor (0x22 0xDF),
    /// which is only valid once something has: right after a reset the
    /// first one reads it (0xFF).
    temperature_loaded: bool,
}

impl<SPI, DC, RST, CS, BUSY, DELAY, POWER> Epaper397<SPI, DC, RST, CS, BUSY, DELAY, POWER>
where
    SPI: SpiBus<u8>,
    SPI::Error: Debug,
    DC: OutputPin,
    DC::Error: Debug,
    RST: OutputPin,
    RST::Error: Debug,
    CS: OutputPin,
    CS::Error: Debug,
    BUSY: InputPin,
    BUSY::Error: Debug,
    DELAY: DelayNs,
    POWER: PanelPower,
{
    /// Construct the driver and match the reference firmware's idle pin state.
    pub fn new(
        spi: SPI,
        mut dc: DC,
        mut reset: RST,
        mut cs: CS,
        busy: BUSY,
        delay: DELAY,
        power: POWER,
    ) -> Result<Self> {
        dc.set_low()
            .map_err(|error| anyhow!("EPD_DC low failed: {error:?}"))?;
        reset
            .set_high()
            .map_err(|error| anyhow!("EPD_RST high failed: {error:?}"))?;
        // The uploaded reference keeps the panel selected and uses GPIO10 as
        // a manually-owned CS line. Preserve that behavior.
        cs.set_low()
            .map_err(|error| anyhow!("EPD_CS low failed: {error:?}"))?;

        Ok(Self {
            spi,
            dc,
            reset,
            cs,
            busy,
            delay,
            power,
            temperature_loaded: false,
        })
    }

    /// Power the panel and configure the controller for global refresh.
    pub fn initialize(&mut self) -> Result<()> {
        self.begin_initialize()?;
        self.delay.delay_ms(RESET_RECOVERY_MS);
        self.finish_initialize()
    }

    /// First half of [`Self::initialize`]: power the panel rail and pulse
    /// the controller's hardware reset, then return without waiting for the
    /// controller to come out of reset. Boot calls this early and runs
    /// unrelated work (SD config loads, sensor bring-up) during the
    /// controller's own reset time instead of sleeping through it; the caller
    /// must let at least `RESET_RECOVERY_MS` pass before
    /// [`Self::finish_initialize`], which then only has to confirm BUSY.
    pub fn begin_initialize(&mut self) -> Result<()> {
        info!("epd397: enable ALDO3 and initialize panel");
        let rail_span = crate::boot_profile::span("epd-rail-enable");
        self.power.enable_panel_rail()?;
        self.delay.delay_ms(10);
        rail_span.end();
        let _reset_span = crate::boot_profile::span("epd-hardware-reset-pulse");
        self.hardware_reset_pulse()
    }

    /// Second half of [`Self::initialize`]; see [`Self::begin_initialize`].
    pub fn finish_initialize(&mut self) -> Result<()> {
        let reset_wait_span = crate::boot_profile::span("epd-reset-busy-wait");
        self.wait_until_idle()?;
        reset_wait_span.end();

        let swreset_span = crate::boot_profile::span("epd-swreset");
        self.command(0x12)?; // SWRESET
        self.wait_until_idle()?;
        self.temperature_loaded = false;
        swreset_span.end();
        let _config_span = crate::boot_profile::span("epd-controller-config");

        self.command_data(0x18, &[0x80])?;
        self.command_data(0x0C, &[0xAE, 0xC7, 0xC3, 0xC0, 0x80])?;
        self.command_data(
            0x01,
            &[((HEIGHT - 1) & 0xFF) as u8, ((HEIGHT - 1) >> 8) as u8, 0x02],
        )?;
        self.command_data(0x3C, &[0x01])?;
        self.command_data(0x11, &[0x01])?;
        self.command_data(
            0x44,
            &[
                0x00,
                0x00,
                ((WIDTH - 1) & 0xFF) as u8,
                ((WIDTH - 1) >> 8) as u8,
            ],
        )?;
        self.command_data(
            0x45,
            &[
                ((HEIGHT - 1) & 0xFF) as u8,
                ((HEIGHT - 1) >> 8) as u8,
                0x00,
                0x00,
            ],
        )?;
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.wait_until_idle()
    }

    /// Transfer a base frame to both controller RAM planes and run a global
    /// refresh. Use this at boot and periodically after partial refreshes.
    pub fn show_base(&mut self, frame: &[u8]) -> Result<()> {
        validate_frame(frame)?;
        debug!("epd397: global base refresh");
        let transfer_span = crate::boot_profile::span("epd-global-spi-transfer");
        self.command(0x24)?;
        self.data(frame)?;
        self.command(0x26)?;
        self.data(frame)?;
        transfer_span.end();
        let _refresh_span = crate::boot_profile::span("epd-global-refresh-wait");
        self.turn_on_display(0xF7)?;
        self.temperature_loaded = true;
        Ok(())
    }

    /// Transfer a base frame to both controller RAM planes and run a global
    /// refresh using the fast waveform (0x22 0xD7). 0xD7 is 0xF7 without the
    /// "load temperature from sensor" step, so 0x1A is written explicitly
    /// right before it to select the OTP's fast-waveform LUT bucket
    /// (`FAST_GLOBAL_TEMPERATURE`) instead of whatever the sensor last put
    /// there — this drops the standard multi-flash waveform down to a
    /// single white-to-black flash. Partial refreshes (0xFF/0xDF) are
    /// unaffected: they select their own LUT bucket independently of 0x1A.
    pub fn show_base_fast(&mut self, frame: &[u8]) -> Result<()> {
        validate_frame(frame)?;
        debug!("epd397: global base refresh (fast waveform)");
        let _span = crate::boot_profile::span("epd-fast-global-refresh");
        self.command_data(0x3C, &[0x01])?;
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.command(0x24)?;
        self.data(frame)?;
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.command(0x26)?;
        self.data(frame)?;
        self.command_data(0x1A, &[FAST_GLOBAL_TEMPERATURE])?;
        self.turn_on_display(0xD7)?;
        self.temperature_loaded = true;
        Ok(())
    }

    /// Write `frame` to both controller RAM planes without refreshing the
    /// glass. After the panel's idle sleep (controller in deep sleep, rail
    /// off) the image is still on the glass, but the RAM the partial refresh
    /// compares against is lost: loading the frame the glass shows puts it
    /// back, and the next [`Self::show_partial_fullscreen`] then changes
    /// only what changes, instead of the global refresh (a flash on nearly
    /// every page turn after a minute of reading) that was needed before.
    pub fn load_base_silent(&mut self, frame: &[u8]) -> Result<()> {
        validate_frame(frame)?;
        debug!("epd397: silent base load");
        let _span = crate::boot_profile::span("epd-silent-base-load");
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.command(0x24)?;
        self.data(frame)?;
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.command(0x26)?;
        self.data(frame)
    }

    /// Apply a full-screen partial refresh. This intentionally mirrors the
    /// vendor UI behavior while keeping the API narrow for the first milestone.
    ///
    /// Does not toggle EPD_RST: the controller is already initialized (once,
    /// in `initialize`) and a full hardware reset before every partial
    /// refresh only adds a fixed ~102 ms of delay (plus reloading the
    /// default waveform) without being needed to reconfigure partial mode,
    /// which the commands below already do on every call.
    ///
    /// Skips the controller's temperature reload (0x22 bit 0x20): re-reading
    /// the sensor on every partial refresh spends time on a value that can't
    /// have drifted between two UI updates. `show_base` still reloads it on
    /// every global refresh (boot, periodic ghost cleanup, manual cleanup,
    /// safety fallback), and the first partial after a controller reset
    /// reads it itself (0xFF), which bounds staleness to at most
    /// `PANEL_PARTIAL_REFRESH_LIMIT` partials or one idle-sleep interval.
    pub fn show_partial_fullscreen(&mut self, frame: &[u8]) -> Result<()> {
        validate_frame(frame)?;
        debug!("epd397: partial full-screen refresh");
        let _span = crate::boot_profile::span("epd-partial-refresh");
        self.command_data(0x18, &[0x80])?;
        self.command_data(0x3C, &[0x80])?;
        self.command_data(0x44, &[0x00, 0x00, 0x18, 0x03])?; // 0 .. 792
        self.command_data(0x45, &[0xDF, 0x01, 0x00, 0x00])?; // 479 .. 0
        self.command_data(0x4E, &[0x00, 0x00])?;
        self.command_data(0x4F, &[0x00, 0x00])?;
        self.command(0x24)?;
        self.data(frame)?;
        let control = if self.temperature_loaded { 0xDF } else { 0xFF };
        self.turn_on_display(control)?;
        self.temperature_loaded = true;
        Ok(())
    }

    /// Put the panel controller into deep sleep and disable its PMIC rail.
    pub fn sleep(&mut self) -> Result<()> {
        info!("epd397: deep sleep and disable ALDO3");
        self.command_data(0x10, &[0x01])?;
        self.delay.delay_ms(10);
        self.reset
            .set_low()
            .map_err(|error| anyhow!("EPD_RST low failed: {error:?}"))?;
        self.cs
            .set_low()
            .map_err(|error| anyhow!("EPD_CS low failed: {error:?}"))?;
        self.dc
            .set_low()
            .map_err(|error| anyhow!("EPD_DC low failed: {error:?}"))?;
        self.power.disable_panel_rail()?;
        self.delay.delay_ms(10);
        Ok(())
    }

    /// Reset pulse without the vendor sequence's trailing recovery delay,
    /// which callers apply themselves (`RESET_RECOVERY_MS`).
    fn hardware_reset_pulse(&mut self) -> Result<()> {
        self.reset
            .set_high()
            .map_err(|error| anyhow!("EPD_RST high failed: {error:?}"))?;
        self.delay.delay_ms(50);
        self.reset
            .set_low()
            .map_err(|error| anyhow!("EPD_RST low failed: {error:?}"))?;
        self.delay.delay_ms(2);
        self.reset
            .set_high()
            .map_err(|error| anyhow!("EPD_RST high failed: {error:?}"))
    }

    fn turn_on_display(&mut self, refresh_control: u8) -> Result<()> {
        self.command_data(0x22, &[refresh_control])?;
        self.command(0x20)?;
        self.wait_until_idle()
    }

    fn wait_until_idle(&mut self) -> Result<()> {
        self.delay.delay_ms(BUSY_SETTLE_MS);
        let mut elapsed_ms = BUSY_SETTLE_MS;
        while self
            .busy
            .is_high()
            .map_err(|error| anyhow!("EPD_BUSY read failed: {error:?}"))?
        {
            if elapsed_ms >= BUSY_TIMEOUT_MS {
                bail!("EPD_BUSY remained high for {BUSY_TIMEOUT_MS} ms");
            }
            self.delay.delay_ms(BUSY_POLL_MS);
            elapsed_ms += BUSY_POLL_MS;
        }
        debug!("epd397: busy released after {elapsed_ms} ms");
        Ok(())
    }

    fn command_data(&mut self, command: u8, data: &[u8]) -> Result<()> {
        self.command(command)?;
        self.data(data)
    }

    fn command(&mut self, command: u8) -> Result<()> {
        self.dc
            .set_low()
            .map_err(|error| anyhow!("EPD_DC command mode failed: {error:?}"))?;
        self.spi
            .write(&[command])
            .map_err(|error| anyhow!("EPD command 0x{command:02X} failed: {error:?}"))
    }

    fn data(&mut self, data: &[u8]) -> Result<()> {
        self.dc
            .set_high()
            .map_err(|error| anyhow!("EPD_DC data mode failed: {error:?}"))?;
        self.spi.write(data).map_err(|error| {
            anyhow!(
                "EPD data transfer of {} bytes failed: {error:?}",
                data.len()
            )
        })
    }
}

fn validate_frame(frame: &[u8]) -> Result<()> {
    if frame.len() != FRAMEBUFFER_SIZE {
        bail!(
            "invalid panel frame size: got {}, expected {FRAMEBUFFER_SIZE}",
            frame.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_frame;
    use crate::framebuffer::FRAMEBUFFER_SIZE;

    #[test]
    fn accepts_native_frame_size() {
        let frame = vec![0_u8; FRAMEBUFFER_SIZE];
        assert!(validate_frame(&frame).is_ok());
    }

    #[test]
    fn rejects_wrong_frame_size() {
        assert!(validate_frame(&[]).is_err());
        assert!(validate_frame(&[0_u8; 1]).is_err());
    }
}
