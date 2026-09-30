//! ESP-IDF playback runtime for the ES8311 codec on the I2S0 TX channel.

use anyhow::{anyhow, Result};
use embedded_hal::{delay::DelayNs, i2c::I2c};
use es8311::{ClockConfig, Resolution};
use esp_idf_svc::hal::{
    delay::BLOCK,
    gpio::{Output, PinDriver},
    i2s::{I2sDriver, I2sTx},
};

use super::{
    board_codec::{BoardEs8311, CodecProfileSnapshot},
    tone::{ChimeGenerator, ChimeMode, PCM_CHUNK_BYTES},
    AudioPlaybackState, AudioSnapshot, AudioUiRequest, AUDIO_MCLK_HZ, AUDIO_SAMPLE_RATE_HZ,
    AUDIO_VOLUME_STEP_PERCENT, DEFAULT_AUDIO_VOLUME_PERCENT, ES8311_I2C_ADDRESS_HIGH,
    ES8311_I2C_ADDRESS_LOW, MAX_AUDIO_VOLUME_PERCENT,
};

/// Own the safe-start playback runtime. The amplifier is held low unless PCM
/// audio is actively being streamed.
pub struct AudioRuntime<'d, I2C> {
    bus: I2C,
    codec: BoardEs8311,
    profile: CodecProfileSnapshot,
    tx: I2sDriver<'d, I2sTx>,
    amplifier: PinDriver<'d, Output>,
    snapshot: AudioSnapshot,
    chime: ChimeGenerator,
}

impl<'d, I2C> AudioRuntime<'d, I2C>
where
    I2C: I2c,
    I2C::Error: core::fmt::Debug,
{
    pub fn initialize<D>(
        mut bus: I2C,
        tx: I2sDriver<'d, I2sTx>,
        mut amplifier: PinDriver<'d, Output>,
        delay: &mut D,
    ) -> Result<Self>
    where
        D: DelayNs,
    {
        amplifier
            .set_low()
            .map_err(|error| anyhow!("failed to disable audio amplifier: {error:?}"))?;
        let clock = ClockConfig {
            mclk_inverted: false,
            sclk_inverted: false,
            mclk_from_mclk_pin: true,
            mclk_frequency: AUDIO_MCLK_HZ,
            sample_frequency: AUDIO_SAMPLE_RATE_HZ,
        };
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
        codec
            .volume_set(&mut bus, DEFAULT_AUDIO_VOLUME_PERCENT, None)
            .map_err(|error| anyhow!("failed to set ES8311 volume: {error:?}"))?;
        codec
            .mute(&mut bus, true)
            .map_err(|error| anyhow!("failed to mute ES8311: {error:?}"))?;

        Ok(Self {
            bus,
            codec,
            profile,
            tx,
            amplifier,
            snapshot: AudioSnapshot {
                available: true,
                codec_address: Some(codec_address),
                codec_ready: true,
                i2s_ready: true,
                amplifier_enabled: false,
                muted: true,
                volume_percent: DEFAULT_AUDIO_VOLUME_PERCENT,
                playback_state: AudioPlaybackState::Muted,
                error: None,
            },
            chime: ChimeGenerator::default(),
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

    /// Disable the I2S TX channel. ESP-IDF's I2S driver holds an
    /// `ESP_PM_APB_FREQ_MAX` power-management lock for as long as a channel
    /// is enabled (see `i2s_channel_enable` in
    /// `esp_driver_i2s/i2s_common.c`), which blocks automatic light sleep
    /// entirely -- not just DFS -- regardless of what else in the firmware is
    /// idle. Call this only while nothing is playing. MCLK keeps toggling
    /// into the codec regardless; only the driver's software channel state
    /// changes here.
    pub fn suspend_i2s(&mut self) -> Result<()> {
        self.tx
            .tx_disable()
            .map_err(|error| anyhow!("failed to disable I2S TX channel: {error:?}"))?;
        Ok(())
    }

    /// Re-enable the I2S TX channel after [`Self::suspend_i2s`], restoring
    /// the boot-time channel state (and taking the light-sleep-blocking PM
    /// lock again) before codec traffic resumes.
    pub fn resume_i2s(&mut self) -> Result<()> {
        self.tx
            .tx_enable()
            .map_err(|error| anyhow!("failed to enable I2S TX channel: {error:?}"))?;
        Ok(())
    }

    /// Reprogram the ES8311 after its AXP2101 rail (ALDO2) was cut and
    /// re-enabled for battery savings while Reader was the active screen. A
    /// power cycle resets every ES8311 register to its power-on default, so
    /// this replays the same register sequence [`Self::initialize`] used at
    /// boot -- minus the two-address probe, since the codec address is
    /// already known -- and restores the previously selected volume rather
    /// than resetting it to the boot default. The I2S driver and amplifier
    /// GPIO stayed configured throughout: only the codec chip lost power.
    pub fn reinit_after_rail_restore<D>(&mut self, delay: &mut D) -> Result<()>
    where
        D: DelayNs,
    {
        let clock = ClockConfig {
            mclk_inverted: false,
            sclk_inverted: false,
            mclk_from_mclk_pin: true,
            mclk_frequency: AUDIO_MCLK_HZ,
            sample_frequency: AUDIO_SAMPLE_RATE_HZ,
        };
        self.profile = self.codec.init(
            &mut self.bus,
            &clock,
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

    pub fn apply_request(&mut self, request: AudioUiRequest) -> Result<&'static str> {
        match request {
            AudioUiRequest::PlayTestChime => {
                self.begin_playback(ChimeMode::TestOnce)?;
                Ok("test-tone-start")
            }
            AudioUiRequest::StopPlayback => {
                self.stop_playback()?;
                Ok("playback-stop")
            }
            AudioUiRequest::VolumeUp => {
                self.set_volume(
                    self.snapshot
                        .volume_percent
                        .saturating_add(AUDIO_VOLUME_STEP_PERCENT),
                )?;
                Ok("volume-up")
            }
            AudioUiRequest::VolumeDown => {
                self.set_volume(
                    self.snapshot
                        .volume_percent
                        .saturating_sub(AUDIO_VOLUME_STEP_PERCENT),
                )?;
                Ok("volume-down")
            }
            AudioUiRequest::ToggleMute => {
                self.set_muted(!self.snapshot.muted)?;
                Ok(if self.snapshot.muted {
                    "muted"
                } else {
                    "unmuted"
                })
            }
        }
    }

    /// Feed one bounded DMA chunk. Returns `true` when the visible diagnostics
    /// state changes, for example when a one-shot test chime completes.
    pub fn tick(&mut self) -> Result<bool> {
        if !self.chime.is_playing() {
            return Ok(false);
        }
        let mut bytes = [0_u8; PCM_CHUNK_BYTES];
        let completed_test = self.chime.fill_stereo_pcm(
            &mut bytes,
            self.snapshot.volume_percent,
            self.snapshot.muted,
        );
        self.tx
            .write_all(&bytes, BLOCK)
            .map_err(|error| anyhow!("I2S TX write failed: {error:?}"))?;
        if completed_test {
            self.stop_playback()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn stop_playback(&mut self) -> Result<()> {
        self.chime.stop();
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
        self.chime.stop();
        self.snapshot.amplifier_enabled = false;
        self.snapshot.muted = true;
        self.snapshot.playback_state = AudioPlaybackState::Error;
        self.snapshot.error = Some(error.into());
    }

    fn begin_playback(&mut self, mode: ChimeMode) -> Result<()> {
        match mode {
            ChimeMode::TestOnce => self.chime.start_test_once(),
            ChimeMode::Idle => self.chime.stop(),
        }
        self.codec
            .mute(&mut self.bus, false)
            .map_err(|error| anyhow!("failed to unmute ES8311: {error:?}"))?;
        self.amplifier
            .set_high()
            .map_err(|error| anyhow!("failed to enable audio amplifier: {error:?}"))?;
        self.snapshot.muted = false;
        self.snapshot.amplifier_enabled = true;
        self.snapshot.playback_state = match mode {
            ChimeMode::TestOnce => AudioPlaybackState::PlayingTestTone,
            ChimeMode::Idle => AudioPlaybackState::Ready,
        };
        self.snapshot.error = None;
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) -> Result<()> {
        self.codec
            .mute(&mut self.bus, muted)
            .map_err(|error| anyhow!("failed to change ES8311 mute state: {error:?}"))?;
        if muted || !self.chime.is_playing() {
            self.amplifier
                .set_low()
                .map_err(|error| anyhow!("failed to disable audio amplifier: {error:?}"))?;
            self.snapshot.amplifier_enabled = false;
        } else {
            self.amplifier
                .set_high()
                .map_err(|error| anyhow!("failed to enable audio amplifier: {error:?}"))?;
            self.snapshot.amplifier_enabled = true;
        }
        self.snapshot.muted = muted;
        self.snapshot.playback_state = if self.chime.mode() == ChimeMode::TestOnce {
            AudioPlaybackState::PlayingTestTone
        } else if muted {
            AudioPlaybackState::Muted
        } else {
            AudioPlaybackState::Ready
        };
        Ok(())
    }

    fn set_volume(&mut self, requested: u8) -> Result<()> {
        let volume = requested.min(MAX_AUDIO_VOLUME_PERCENT);
        self.codec
            .volume_set(&mut self.bus, volume, None)
            .map_err(|error| anyhow!("failed to set ES8311 volume: {error:?}"))?;
        self.snapshot.volume_percent = volume;
        Ok(())
    }
}
