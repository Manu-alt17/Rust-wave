//! ES8311 playback controls and readable board-profile details.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
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
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let audio = &state.audio;
    let volume = format!("{}%", audio.volume_percent);
    let amp = if audio.amplifier_enabled {
        t(locale, "ON", "ON")
    } else {
        t(locale, "OFF", "OFF")
    };
    let mute = if audio.muted {
        t(locale, "Muted", "Silenziato")
    } else {
        t(locale, "Active", "Attivo")
    };

    draw_header(display, state, t(locale, "AUDIO", "AUDIO"))?;

    Text::new(
        t(locale, "Playback controls", "Controlli di riproduzione"),
        Point::new(22, 112),
        heading,
    )
    .draw(display)?;
    line(display, 156, t(locale, "Status", "Stato"), mute, body)?;
    line(display, 190, t(locale, "Volume", "Volume"), &volume, body)?;
    line(
        display,
        224,
        t(locale, "Amplifier", "Amplificatore"),
        amp,
        body,
    )?;

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
        t(locale, "Audio details", "Dettagli audio"),
    ];
    for (index, label) in labels.into_iter().enumerate() {
        draw_action(
            display,
            272 + index as i32 * 58,
            label,
            state.audio_action_selected == index,
            body,
        )?;
    }
    draw_footer(display, state, t(locale, "SELECT RUN", "SELECT ESEGUI"))?;
    Ok(())
}

pub fn render_audio_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let audio = &state.audio;
    let address = audio.codec_address_label();
    let volume = format!("{}%", audio.volume_percent);
    let amp = if audio.amplifier_enabled {
        t(locale, "ON", "ON")
    } else {
        t(locale, "OFF", "OFF")
    };
    let mute = if audio.muted {
        t(locale, "MUTED", "SILENZIATO")
    } else {
        t(locale, "ACTIVE", "ATTIVO")
    };
    let sample_rate = format!("{AUDIO_SAMPLE_RATE_HZ} Hz");
    let tx_pins =
        format!("M{AUDIO_MCLK_GPIO} B{AUDIO_BCLK_GPIO} W{AUDIO_WS_GPIO} D{AUDIO_DOUT_GPIO}");
    let rx_input = match locale {
        Locale::English => format!("DIN GPIO{AUDIO_DIN_GPIO} deferred"),
        Locale::Italian => format!("DIN GPIO{AUDIO_DIN_GPIO} rinviato"),
    };
    let amplifier_pin = format!("GPIO{AUDIO_AMP_ENABLE_GPIO} {amp}");

    draw_header(display, state, t(locale, "AUDIO INFO", "INFO AUDIO"))?;

    Text::new(t(locale, "Codec", "Codec"), Point::new(22, 114), heading).draw(display)?;
    line(
        display,
        158,
        t(locale, "Device", "Dispositivo"),
        "ES8311 BSP-REF58",
        body,
    )?;
    line(
        display,
        192,
        t(locale, "Address", "Indirizzo"),
        &address,
        body,
    )?;
    line(
        display,
        226,
        t(locale, "I2S mode", "Modalità I2S"),
        "TX ONLY / S16 STEREO",
        body,
    )?;
    line(
        display,
        260,
        t(locale, "Sample rate", "Frequenza di campionamento"),
        &sample_rate,
        body,
    )?;

    Text::new(
        t(locale, "Routing", "Instradamento"),
        Point::new(22, 324),
        heading,
    )
    .draw(display)?;
    line(display, 368, t(locale, "TX pins", "Pin TX"), &tx_pins, body)?;
    line(
        display,
        402,
        t(locale, "RX input", "Ingresso RX"),
        &rx_input,
        body,
    )?;
    line(
        display,
        436,
        t(locale, "Amplifier", "Amplificatore"),
        &amplifier_pin,
        body,
    )?;
    line(display, 470, t(locale, "Mute", "Silenzia"), mute, body)?;
    line(display, 504, t(locale, "Volume", "Volume"), &volume, body)?;

    Text::new(
        t(locale, "Last error", "Ultimo errore"),
        Point::new(22, 568),
        heading,
    )
    .draw(display)?;
    Text::new(
        audio
            .error
            .as_deref()
            .unwrap_or(t(locale, "none", "nessuno")),
        Point::new(22, 606),
        detail,
    )
    .draw(display)?;
    Ok(())
}

fn line(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    label: &str,
    value: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(label, Point::new(22, y), style).draw(display)?;
    Text::new(value, Point::new(170, y), style).draw(display)?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 48))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 31),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 31), style).draw(display)?;
    Ok(())
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
