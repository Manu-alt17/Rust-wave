//! Native audio domain for the Waveshare ESP32-S3 e-Paper 3.97 board.
//!
//! Test tone and audiobook playback through the ES8311 on the I2S0 TX
//! channel. Host-testable state lives here; ESP-IDF wiring stays in
//! [`espidf`] (codec and amplifier), [`mp3`] (decoder) and [`engine`]
//! (the playback thread).

pub mod tone;

#[cfg(target_os = "espidf")]
pub mod board_codec;
#[cfg(target_os = "espidf")]
pub mod engine;
#[cfg(target_os = "espidf")]
pub mod espidf;
#[cfg(target_os = "espidf")]
pub mod mp3;

/// ES8311 seven-bit address when the CE strap is low.
pub const ES8311_I2C_ADDRESS_LOW: u8 = 0x18;
/// ES8311 seven-bit address when the CE strap is high.
pub const ES8311_I2C_ADDRESS_HIGH: u8 = 0x19;
/// Eight-bit wire write address commonly printed by board reference material.
pub const ES8311_WIRE_WRITE_ADDRESS_LOW: u8 = ES8311_I2C_ADDRESS_LOW << 1;
/// Test tone sample rate.
pub const AUDIO_SAMPLE_RATE_HZ: u32 = 16_000;
/// MCLK is 256 × the sample rate for every stream, the ratio the codec is
/// set up for (see [`CODEC_REFERENCE_MCLK_HZ`]).
pub const AUDIO_MCLK_MULTIPLE: u32 = 256;
/// Test tone MCLK: 16 kHz × 256 = 4.096 MHz.
pub const AUDIO_MCLK_HZ: u32 = AUDIO_SAMPLE_RATE_HZ * AUDIO_MCLK_MULTIPLE;
/// The ES8311 clock table entry the codec is programmed from. Its dividers
/// depend only on the 256× ratio, identical for every 256× entry in the
/// table, so any sample rate with a 256× MCLK plays through it unchanged.
pub const CODEC_REFERENCE_MCLK_HZ: u32 = 12_288_000;
pub const CODEC_REFERENCE_SAMPLE_RATE_HZ: u32 = 48_000;
/// Match the uploaded Waveshare playback BSP while keeping safe startup mute
/// and amplifier-disable behavior until explicit playback begins.
pub const DEFAULT_AUDIO_VOLUME_PERCENT: u8 = 60;
/// Bounded user-facing audio volume range.
pub const MAX_AUDIO_VOLUME_PERCENT: u8 = 100;
/// Volume adjustment step exposed by the diagnostics UI.
pub const AUDIO_VOLUME_STEP_PERCENT: u8 = 5;
/// I2S TX pin ownership inherited from the uploaded BSP.
pub const AUDIO_MCLK_GPIO: u8 = 13;
pub const AUDIO_BCLK_GPIO: u8 = 14;
pub const AUDIO_WS_GPIO: u8 = 47;
/// ESP32-S3 TX data output to the ES8311 DAC. The uploaded BSP names this
/// signal `I2S_DATA_POUT` and routes it to GPIO48.
pub const AUDIO_DOUT_GPIO: u8 = 48;
/// ES8311 ADC data back to the ESP32-S3. Not claimed: nothing records.
pub const AUDIO_DIN_GPIO: u8 = 21;
pub const AUDIO_AMP_ENABLE_GPIO: u8 = 39;

/// Waveshare sample-app codec-profile values inherited from Espressif's
/// `esp_codec_dev` ES8311 open/start sequence. These are deliberately explicit
/// so a successful I2C probe cannot be mistaken for a working DAC path.
pub const BSP_ES8311_GPIO_IDLE_REG44: u8 = 0x08;
pub const BSP_ES8311_DAC_REFERENCE_REG44: u8 = 0x58;
pub const BSP_ES8311_SYSTEM_REG14: u8 = 0x1A;
pub const BSP_ES8311_ADC_REG15: u8 = 0x40;
pub const BSP_ES8311_ADC_REG17: u8 = 0xBF;
pub const BSP_ES8311_GP_REG45: u8 = 0x00;

/// Product-facing audio state.  Keep this small so it can be copied into UI
/// snapshots without retaining I2C or I2S handles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioPlaybackState {
    #[default]
    Unavailable,
    Muted,
    Ready,
    PlayingTestTone,
    PlayingAudiobook,
    Error,
}

impl AudioPlaybackState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "UNAVAILABLE",
            Self::Muted => "MUTED",
            Self::Ready => "READY",
            Self::PlayingTestTone => "TEST TONE",
            Self::PlayingAudiobook => "AUDIOBOOK",
            Self::Error => "ERROR",
        }
    }
}

/// Password-free, handle-free audio state rendered by the product shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioSnapshot {
    pub available: bool,
    pub codec_address: Option<u8>,
    pub codec_ready: bool,
    pub i2s_ready: bool,
    pub amplifier_enabled: bool,
    pub muted: bool,
    pub volume_percent: u8,
    pub playback_state: AudioPlaybackState,
    pub error: Option<String>,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self::unavailable("audio subsystem has not been initialized")
    }
}

impl AudioSnapshot {
    #[must_use]
    pub fn unavailable(error: impl Into<String>) -> Self {
        Self {
            available: false,
            codec_address: None,
            codec_ready: false,
            i2s_ready: false,
            amplifier_enabled: false,
            muted: true,
            volume_percent: DEFAULT_AUDIO_VOLUME_PERCENT,
            playback_state: AudioPlaybackState::Unavailable,
            error: Some(error.into()),
        }
    }

    #[must_use]
    pub const fn home_badge(&self) -> &'static str {
        match self.playback_state {
            AudioPlaybackState::Unavailable => "NO AUD",
            AudioPlaybackState::Muted => "MUTED",
            AudioPlaybackState::Ready => "READY",
            AudioPlaybackState::PlayingTestTone => "TEST",
            AudioPlaybackState::PlayingAudiobook => "BOOK",
            AudioPlaybackState::Error => "ERROR",
        }
    }

    #[must_use]
    pub fn codec_address_label(&self) -> String {
        self.codec_address.map_or_else(
            || "--".into(),
            |address| format!("0x{address:02X} (wire 0x{:02X})", address << 1),
        )
    }
}

/// The volume last chosen on the device, from the audiobook player or the
/// Audio settings. Without it every boot -- and every wake from standby is
/// one -- started again from [`DEFAULT_AUDIO_VOLUME_PERCENT`].
pub const AUDIO_VOLUME_PATH: &str = "/sdcard/RUSTMIX/VOLUME.TXT";

/// The `volume=` value of a saved volume file, within the codec's range.
#[must_use]
pub fn parse_saved_volume(text: &str) -> Option<u8> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("volume="))
        .find_map(|value| value.trim().parse::<u8>().ok())
        .map(|volume| volume.min(MAX_AUDIO_VOLUME_PERCENT))
}

#[must_use]
pub fn serialize_saved_volume(volume: u8) -> String {
    format!(
        "# RustMix Wave audio volume, 0-{MAX_AUDIO_VOLUME_PERCENT}\nvolume={}\n",
        volume.min(MAX_AUDIO_VOLUME_PERCENT)
    )
}

/// The saved volume, or the default when there is none (first boot, no
/// card, an unreadable file).
#[must_use]
pub fn load_saved_volume(path: &std::path::Path) -> u8 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| parse_saved_volume(&text))
        .unwrap_or(DEFAULT_AUDIO_VOLUME_PERCENT)
}

/// Written to a temporary file first, then renamed over the old one, so a
/// power cut mid-write leaves the previous volume.
pub fn save_volume(path: &std::path::Path, volume: u8) -> std::io::Result<()> {
    let temporary = path.with_extension("TMP");
    std::fs::write(&temporary, serialize_saved_volume(volume))?;
    // FatFs refuses to rename onto an existing file.
    let _ = std::fs::remove_file(path);
    std::fs::rename(&temporary, path)
}

/// Hardware-independent requests produced by the Audio diagnostics screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioUiRequest {
    PlayTestChime,
    StopPlayback,
    VolumeUp,
    VolumeDown,
    ToggleMute,
}

#[cfg(test)]
mod tests {
    use super::{
        AudioPlaybackState, AudioSnapshot, AUDIO_DIN_GPIO, AUDIO_DOUT_GPIO, BSP_ES8311_ADC_REG15,
        BSP_ES8311_ADC_REG17, BSP_ES8311_DAC_REFERENCE_REG44, BSP_ES8311_GP_REG45,
        BSP_ES8311_SYSTEM_REG14, DEFAULT_AUDIO_VOLUME_PERCENT, ES8311_I2C_ADDRESS_LOW,
        ES8311_WIRE_WRITE_ADDRESS_LOW,
    };

    #[test]
    fn seven_bit_and_wire_write_addresses_are_explicit() {
        assert_eq!(ES8311_I2C_ADDRESS_LOW, 0x18);
        assert_eq!(ES8311_WIRE_WRITE_ADDRESS_LOW, 0x30);
    }

    #[test]
    fn uploaded_bsp_data_route_keeps_tx_and_unused_rx_explicit() {
        assert_eq!(AUDIO_DOUT_GPIO, 48);
        assert_eq!(AUDIO_DIN_GPIO, 21);
    }

    #[test]
    fn waveshare_codec_profile_keeps_dac_reference_explicit() {
        assert_eq!(BSP_ES8311_DAC_REFERENCE_REG44, 0x58);
        assert_eq!(BSP_ES8311_SYSTEM_REG14, 0x1A);
        assert_eq!(BSP_ES8311_ADC_REG15, 0x40);
        assert_eq!(BSP_ES8311_ADC_REG17, 0xBF);
        assert_eq!(BSP_ES8311_GP_REG45, 0x00);
        assert_eq!(DEFAULT_AUDIO_VOLUME_PERCENT, 60);
    }

    #[test]
    fn the_volume_survives_a_save_and_a_load() {
        let dir = std::env::temp_dir().join(format!("rustmix-volume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("VOLUME.TXT");
        assert_eq!(
            super::load_saved_volume(&path),
            DEFAULT_AUDIO_VOLUME_PERCENT
        );
        super::save_volume(&path, 35).unwrap();
        assert_eq!(super::load_saved_volume(&path), 35);
        super::save_volume(&path, 0).unwrap();
        assert_eq!(super::load_saved_volume(&path), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_volume_parsing_is_lenient_and_bounded() {
        assert_eq!(
            super::parse_saved_volume("# comment\nvolume=45\n"),
            Some(45)
        );
        assert_eq!(super::parse_saved_volume(" volume= 70 "), Some(70));
        assert_eq!(super::parse_saved_volume("volume=250"), Some(100));
        assert_eq!(super::parse_saved_volume("volume=loud\n"), None);
        assert_eq!(super::parse_saved_volume(""), None);
        let saved = super::serialize_saved_volume(55);
        assert_eq!(super::parse_saved_volume(&saved), Some(55));
    }

    #[test]
    fn unavailable_audio_reports_no_audio() {
        let snapshot = AudioSnapshot::unavailable("codec absent");
        assert_eq!(snapshot.playback_state, AudioPlaybackState::Unavailable);
        assert_eq!(snapshot.home_badge(), "NO AUD");
    }
}
