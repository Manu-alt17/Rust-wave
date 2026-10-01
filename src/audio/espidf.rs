//! ES8311 codec and speaker amplifier control.
//!
//! The PCM stream itself -- the I2S0 TX channel, test tone and audiobook
//! decoding -- belongs to [`super::engine`], which also owns this runtime
//! and drives it from its own thread.

use anyhow::{anyhow, Result};
use embedded_hal::{delay::DelayNs, i2c::I2c};
use es8311::{ClockConfig, Resolution};
use esp_idf_svc::hal::gpio::{Output, PinDriver};

use super::{
    board_codec::{BoardEs8311, CodecProfileSnapshot},
    AudioPlaybackState, AudioSnapshot, CODEC_REFERENCE_MCLK_HZ, CODEC_REFERENCE_SAMPLE_RATE_HZ,
    ES8311_I2C_ADDRESS_HIGH, ES8311_I2C_ADDRESS_LOW, MAX_AUDIO_VOLUME_PERCENT,
};

/// Codec and amplifier state. The amplifier is held low unless PCM audio is
/// actively being streamed.
pub struct AudioRuntime<'d, I2C> {
    bus: I2C,
    codec: BoardEs8311,
    profile: CodecProfileSnapshot,
    amplifier: PinDriver<'d, Output>,
    snapshot: AudioSnapshot,
}

/// The codec's clock setup. Its dividers depend only on the MCLK to sample
/// rate ratio, which the I2S side keeps at 256 for every rate, so one setup
/// serves 8 kHz test tones and 44.1 kHz audiobooks alike.
fn codec_clock() -> ClockConfig {
    ClockConfig {
        mclk_inverted: false,
        sclk_inverted: false,
        mclk_from_mclk_pin: true,
        mclk_frequency: CODEC_REFERENCE_MCLK_HZ,
        sample_frequency: CODEC_REFERENCE_SAMPLE_RATE_HZ,
    }
}

impl<'d, I2C> AudioRuntime<'d, I2C>
where
    I2C: I2c,
    I2C::Error: core::fmt::Debug,
{
    /// Probe and set up the codec, muted, at `volume` (the one saved on the
    /// SD card, see [`super::load_saved_volume`]).
    pub fn initialize<D>(
        mut bus: I2C,
        mut amplifier: PinDriver<'d, Output>,
        delay: &mut D,
        volume: u8,
    ) -> Result<Self>
    where
        D: DelayNs,
    {
        amplifier
            .set_low()
            .map_err(|error| anyhow!("failed to disable audio amplifier: {error:?}"))?;
        let clock = codec_clock();
        let mut last_error = None;
        let mut detected = None;
        for address in [ES8311_I2C_ADDRESS_LOW, ES8311_I2C_ADDRESS_HIGH] {
            let codec = BoardEs8311::new(address);
            match codec.init(
                &mut bus,
                &clock,
                Resolution::Bits16,
                Resolution::Bits16,
                delay,
            ) {
                Ok(profile) => {
                    detected = Some((codec, address, profile));
                    break;
                }
                Err(error) => last_error = Some(format!("{error:?}")),
            }
        }
        let (codec, codec_address, profile) = detected.ok_or_else(|| {
            anyhow!(
                "ES8311 probe failed at 0x{ES8311_I2C_ADDRESS_LOW:02X} and 0x{ES8311_I2C_ADDRESS_HIGH:02X}: {}",
                last_error.unwrap_or_else(|| "unknown codec error".into())
            )
        })?;
        let volume = volume.min(MAX_AUDIO_VOLUME_PERCENT);
        codec
            .volume_set(&mut bus, volume, None)
            .map_err(|error| anyhow!("failed to set ES8311 volume: {error:?}"))?;
        codec
            .mute(&mut bus, true)
            .map_err(|error| anyhow!("failed to mute ES8311: {error:?}"))?;

        Ok(Self {
            bus,
            codec,
            profile,
            amplifier,
            snapshot: AudioSnapshot {
                available: true,
                codec_address: Some(codec_address),
                codec_ready: true,
                i2s_ready: true,
                amplifier_enabled: false,
                muted: true,
                volume_percent: volume,
                playback_state: AudioPlaybackState::Muted,
                error: None,
            },
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> AudioSnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub const fn profile(&self) -> CodecProfileSnapshot {
        self.profile
    }

    /// Reprogram the ES8311 after its AXP2101 rail (ALDO2) was cut and
    /// re-enabled for battery savings while Reader was the active screen. A
    /// power cycle resets every ES8311 register to its power-on default, so
    /// this replays the same register sequence [`Self::initialize`] used at
    /// boot -- minus the two-address probe, since the codec address is
    /// already known -- and restores the previously selected volume rather
    /// than resetting it to the boot default. The amplifier GPIO stayed
    /// configured throughout: only the codec chip lost power.
    pub fn reinit_after_rail_restore<D>(&mut self, delay: &mut D) -> Result<()>
    where
        D: DelayNs,
    {
        self.profile = self.codec.init(
            &mut self.bus,
            &codec_clock(),
            Resolution::Bits16,
            Resolution::Bits16,
            delay,
        )?;
        self.codec
            .volume_set(&mut self.bus, self.snapshot.volume_percent, None)
            .map_err(|error| anyhow!("failed to restore ES8311 volume: {error:?}"))?;
        self.codec
            .mute(&mut self.bus, true)
            .map_err(|error| anyhow!("failed to re-mute ES8311 after rail restore: {error:?}"))?;
        self.amplifier.set_low().map_err(|error| {
            anyhow!("failed to disable audio amplifier after rail restore: {error:?}")
        })?;
        self.snapshot.codec_ready = true;
        self.snapshot.amplifier_enabled = false;
        self.snapshot.muted = true;
        self.snapshot.playback_state = AudioPlaybackState::Muted;
        self.snapshot.error = None;
        Ok(())
    }

    /// Unmute the codec and switch the amplifier on, once PCM is flowing.
    pub fn begin_output(&mut self, state: AudioPlaybackState) -> Result<()> {
        self.codec
            .mute(&mut self.bus, false)
            .map_err(|error| anyhow!("failed to unmute ES8311: {error:?}"))?;
        if let Err(error) = self.amplifier.set_high() {
            let _ = self.codec.mute(&mut self.bus, true);
            return Err(anyhow!("failed to enable audio amplifier: {error:?}"));
        }
        self.snapshot.muted = false;
        self.snapshot.amplifier_enabled = true;
        self.snapshot.playback_state = state;
        self.snapshot.error = None;
        Ok(())
    }

    /// Amplifier off and codec muted, before PCM stops flowing.
    pub fn end_output(&mut self) -> Result<()> {
        self.amplifier
            .set_low()
            .map_err(|error| anyhow!("failed to disable audio amplifier: {error:?}"))?;
        self.codec
            .mute(&mut self.bus, true)
            .map_err(|error| anyhow!("failed to mute ES8311: {error:?}"))?;
        self.snapshot.amplifier_enabled = false;
        self.snapshot.muted = true;
        self.snapshot.playback_state = AudioPlaybackState::Muted;
        Ok(())
    }

    pub fn record_failure(&mut self, error: impl Into<String>) {
        let _ = self.amplifier.set_low();
        let _ = self.codec.mute(&mut self.bus, true);
        self.snapshot.amplifier_enabled = false;
        self.snapshot.muted = true;
        self.snapshot.playback_state = AudioPlaybackState::Error;
        self.snapshot.error = Some(error.into());
    }

    /// Mute or unmute while `playing` says whether PCM is flowing: the
    /// amplifier only comes on for an unmuted, playing stream.
    pub fn set_muted(&mut self, muted: bool, playing: Option<AudioPlaybackState>) -> Result<()> {
        self.codec
            .mute(&mut self.bus, muted)
            .map_err(|error| anyhow!("failed to change ES8311 mute state: {error:?}"))?;
        match playing {
            Some(state) if !muted => {
                self.amplifier
                    .set_high()
                    .map_err(|error| anyhow!("failed to enable audio amplifier: {error:?}"))?;
                self.snapshot.amplifier_enabled = true;
                self.snapshot.playback_state = state;
            }
            _ => {
                self.amplifier
                    .set_low()
                    .map_err(|error| anyhow!("failed to disable audio amplifier: {error:?}"))?;
                self.snapshot.amplifier_enabled = false;
                self.snapshot.playback_state = playing.unwrap_or(if muted {
                    AudioPlaybackState::Muted
                } else {
                    AudioPlaybackState::Ready
                });
            }
        }
        self.snapshot.muted = muted;
        Ok(())
    }

    pub fn set_volume(&mut self, requested: u8) -> Result<()> {
        let volume = requested.min(MAX_AUDIO_VOLUME_PERCENT);
        self.codec
            .volume_set(&mut self.bus, volume, None)
            .map_err(|error| anyhow!("failed to set ES8311 volume: {error:?}"))?;
        self.snapshot.volume_percent = volume;
        Ok(())
    }
}
