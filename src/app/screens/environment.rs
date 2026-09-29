//! SHTC3-backed environment overview and readable sensor details.

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
    orientation::OrientedFrameBuffer,
};

/// Draw temperature and humidity from the onboard SHTC3.
pub fn render_environment(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let large = state.display.large_style();
    let outline = PrimitiveStyle::with_stroke(BinaryColor::On, 2);
    let temperature = state
        .board
        .temperature_label(state.regional.temperature_unit);
    let humidity = state.board.humidity_label();

    draw_header(display, state, t(locale, "ENVIRONMENT", "AMBIENTE"))?;

    Rectangle::new(Point::new(22, 110), Size::new(436, 160))
        .into_styled(outline)
        .draw(display)?;
    Text::new(
        t(locale, "Temperature", "Temperatura"),
        Point::new(42, 152),
        heading,
    )
    .draw(display)?;
    Text::new(&temperature, Point::new(42, 224), large).draw(display)?;

    Rectangle::new(Point::new(22, 306), Size::new(436, 160))
        .into_styled(outline)
        .draw(display)?;
    Text::new(
        t(locale, "Relative humidity", "Umidità relativa"),
        Point::new(42, 348),
        heading,
    )
    .draw(display)?;
    Text::new(&humidity, Point::new(42, 420), large).draw(display)?;

    draw_action(
        display,
        594,
        t(locale, "Sensor details", "Dettagli sensore"),
        body,
    )?;
    draw_footer(
        display,
        state,
        t(locale, "SELECT DETAILS", "SELECT DETTAGLI"),
    )?;
    Ok(())
}

pub fn render_environment_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let id = state.board.environment_sensor_id.map_or_else(
        || t(locale, "Unavailable", "Non disponibile").into(),
        |id| format!("0x{id:04X}"),
    );

    draw_header(
        display,
        state,
        t(locale, "SENSOR INFO", "INFO SENSORE"),
    )?;

    Text::new(t(locale, "Sensor", "Sensore"), Point::new(22, 120), heading).draw(display)?;
    line(
        display,
        168,
        t(locale, "Device ID", "ID dispositivo"),
        &id,
        body,
    )?;
    line(
        display,
        208,
        t(locale, "Command", "Comando"),
        t(
            locale,
            "Wake / measure / sleep",
            "Attivazione / misura / sospensione",
        ),
        body,
    )?;
    line(
        display,
        248,
        t(locale, "Validation", "Convalida"),
        "Sensirion CRC-8",
        body,
    )?;

    Text::new(
        t(locale, "Calibration", "Calibrazione"),
        Point::new(22, 324),
        heading,
    )
    .draw(display)?;
    line(
        display,
        372,
        t(locale, "Compensation", "Compensazione"),
        "-1.5 C / -2.7 F",
        body,
    )?;
    line(
        display,
        412,
        t(locale, "Live refresh", "Aggiornamento in tempo reale"),
        t(locale, "Every 30 seconds", "Ogni 30 secondi"),
        body,
    )?;

    Text::new(
        t(
            locale,
            "Press BOOT to return to Environment.",
            "Premere BOOT per tornare a Ambiente.",
        ),
        Point::new(22, 580),
        body,
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
    Text::new(value, Point::new(188, y), style).draw(display)?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 4))
        .draw(display)?;
    Text::new(">", Point::new(38, top + 34), style).draw(display)?;
    Text::new(label, Point::new(68, top + 34), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_environment, render_environment_details};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn environment_overview_and_details_render_without_sensor() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_environment(&mut display, &state).unwrap();
        render_environment_details(&mut display, &state).unwrap();
    }
}
