//! ES8311 playback controls and readable board-profile details.

use core::convert::Infallible;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{back_only, draw_footer, select_and_back},
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE},
            list::{draw_field, draw_list_row, draw_section_title, ROW_STEP},
            text::draw_paragraph,
        },
    },
    audio::{
        AUDIO_AMP_ENABLE_GPIO, AUDIO_BCLK_GPIO, AUDIO_DIN_GPIO, AUDIO_DOUT_GPIO, AUDIO_MCLK_GPIO,
        AUDIO_SAMPLE_RATE_HZ, AUDIO_WS_GPIO,
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

pub fn render_audio(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let audio = &state.audio;
    let volume = format!("{}%", audio.volume_percent);
    let mute = if audio.muted {
        t(locale, "Muted", "Silenziato")
    } else {
        t(locale, "On", "Attivo")
    };

    draw_header(display, state, "AUDIO")?;

    let mut baseline = draw_field(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Sound", "Audio"),
        mute,
    )?;
    baseline = draw_field(display, preferences, baseline, "Volume", &volume)?;

    let rows_top = baseline - i32::from(preferences.body_style().line_height()) + 10;
    let labels = [
        t(locale, "Play test chime", "Riproduci suono di prova"),
        t(locale, "Stop playback", "Interrompi riproduzione"),
        t(locale, "Increase volume", "Aumenta volume"),
        t(locale, "Decrease volume", "Diminuisci volume"),
        if audio.muted {
            t(locale, "Unmute", "Riattiva audio")
        } else {
            t(locale, "Mute", "Silenzia")
        },
        t(locale, "Details", "Dettagli"),
    ];
    for (index, label) in labels.into_iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            rows_top + index as i32 * ROW_STEP,
            label,
            "",
            state.audio_action_selected == index,
        )?;
    }
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "RUN", "ESEGUI")),
    )
}

pub fn render_audio_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let audio = &state.audio;
    let address = audio.codec_address_label();
    let volume = format!("{}%", audio.volume_percent);
    let amp = if audio.amplifier_enabled { "ON" } else { "OFF" };
    let mute = if audio.muted {
        t(locale, "Muted", "Silenziato")
    } else {
        t(locale, "On", "Attivo")
    };
    let sample_rate = format!("{AUDIO_SAMPLE_RATE_HZ} Hz");
    let tx_pins =
        format!("M{AUDIO_MCLK_GPIO} B{AUDIO_BCLK_GPIO} W{AUDIO_WS_GPIO} D{AUDIO_DOUT_GPIO}");
    let rx_input = match locale {
        Locale::English => format!("GPIO{AUDIO_DIN_GPIO}, unused"),
        Locale::Italian => format!("GPIO{AUDIO_DIN_GPIO}, non usato"),
    };
    let amplifier_pin = format!("GPIO{AUDIO_AMP_ENABLE_GPIO} {amp}");

    draw_header(display, state, t(locale, "AUDIO INFO", "INFO AUDIO"))?;

    let mut baseline = draw_section_title(display, preferences, FIRST_BASELINE, "Codec")?;
    let codec: [(&str, &str); 4] = [
        (t(locale, "Device", "Dispositivo"), "ES8311"),
        (t(locale, "Address", "Indirizzo"), &address),
        (
            t(locale, "I2S mode", "Modalit\u{00E0} I2S"),
            "TX, S16 stereo",
        ),
        (
            t(locale, "Sample rate", "Frequenza di campionamento"),
            &sample_rate,
        ),
    ];
    for (label, value) in codec {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Routing", "Collegamenti"),
    )?;
    let routing: [(&str, &str); 5] = [
        (t(locale, "TX pins", "Pin TX"), &tx_pins),
        (t(locale, "RX input", "Ingresso RX"), &rx_input),
        (t(locale, "Amplifier", "Amplificatore"), &amplifier_pin),
        (t(locale, "Sound", "Audio"), mute),
        ("Volume", &volume),
    ];
    for (label, value) in routing {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Last error", "Ultimo errore"),
    )?;
    draw_paragraph(
        display,
        audio
            .error
            .as_deref()
            .unwrap_or(t(locale, "None", "Nessuno")),
        CONTENT_LEFT,
        baseline,
        preferences.body_style(),
        CONTENT_WIDTH,
        4,
        4,
    )?;
    draw_footer(display, state, &back_only(locale))
}

#[cfg(test)]
mod tests {
    use super::{render_audio, render_audio_details};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn audio_overview_and_details_render_when_codec_is_unavailable() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_audio(&mut display, &state).unwrap();
        render_audio_details(&mut display, &state).unwrap();
    }
}
