//! ES8311 playback controls and readable board-profile details.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    audio::{
        AUDIO_AMP_ENABLE_GPIO, AUDIO_BCLK_GPIO, AUDIO_DIN_GPIO, AUDIO_DOUT_GPIO, AUDIO_MCLK_GPIO,
        AUDIO_SAMPLE_RATE_HZ, AUDIO_WS_GPIO,
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_audio(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let audio = &state.audio;
    let volume = format!("{}%", audio.volume_percent);
    let amp = if audio.amplifier_enabled { "ON" } else { "OFF" };
    let mute = if audio.muted { "Muted" } else { "Active" };

    draw_header(display, state, "AUDIO")?;

    Text::new("Playback controls", Point::new(22, 112), heading).draw(display)?;
    line(display, 156, "Status", mute, body)?;
    line(display, 190, "Volume", &volume, body)?;
    line(display, 224, "Amplifier", amp, body)?;

    let labels = [
        "Play test chime",
        "Stop playback",
        "Increase volume",
        "Decrease volume",
        if audio.muted { "Unmute" } else { "Mute" },
        "Audio details",
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
    draw_footer(display, state.display, "UP/DOWN  SELECT RUN  BOOT BACK")?;
    Ok(())
}

pub fn render_audio_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let audio = &state.audio;
    let address = audio.codec_address_label();
    let volume = format!("{}%", audio.volume_percent);
    let amp = if audio.amplifier_enabled { "ON" } else { "OFF" };
    let mute = if audio.muted { "MUTED" } else { "ACTIVE" };

    draw_header(display, state, "AUDIO DETAILS")?;

    Text::new("Codec", Point::new(22, 114), heading).draw(display)?;
    line(display, 158, "Device", "ES8311 BSP-REF58", body)?;
    line(display, 192, "Address", &address, body)?;
    line(display, 226, "I2S mode", "TX ONLY / S16 STEREO", body)?;
    line(
        display,
        260,
        "Sample rate",
        &format!("{AUDIO_SAMPLE_RATE_HZ} Hz"),
        body,
    )?;

    Text::new("Routing", Point::new(22, 324), heading).draw(display)?;
    line(
        display,
        368,
        "TX pins",
        &format!("M{AUDIO_MCLK_GPIO} B{AUDIO_BCLK_GPIO} W{AUDIO_WS_GPIO} D{AUDIO_DOUT_GPIO}"),
        body,
    )?;
    line(
        display,
        402,
        "RX input",
        &format!("DIN GPIO{AUDIO_DIN_GPIO} deferred"),
        body,
    )?;
    line(
        display,
        436,
        "Amplifier",
        &format!("GPIO{AUDIO_AMP_ENABLE_GPIO} {amp}"),
        body,
    )?;
    line(display, 470, "Mute", mute, body)?;
    line(display, 504, "Volume", &volume, body)?;

    Text::new("Last error", Point::new(22, 568), heading).draw(display)?;
    Text::new(
        audio.error.as_deref().unwrap_or("none"),
        Point::new(22, 606),
        detail,
    )
    .draw(display)?;
    draw_footer(display, state.display, "BOOT BACK")?;
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
