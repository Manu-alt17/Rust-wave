//! SHTC3-backed environment overview and readable sensor details.

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
    orientation::OrientedFrameBuffer,
};

/// Draw temperature and humidity from the onboard SHTC3.
pub fn render_environment(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let large = state.display.large_style();
    let outline = PrimitiveStyle::with_stroke(BinaryColor::On, 2);
    let temperature = state
        .board
        .temperature_label(state.regional.temperature_unit);
    let humidity = state.board.humidity_label();

    draw_header(display, state, "ENVIRONMENT")?;

    Rectangle::new(Point::new(22, 110), Size::new(436, 160))
        .into_styled(outline)
        .draw(display)?;
    Text::new("Temperature", Point::new(42, 152), heading).draw(display)?;
    Text::new(&temperature, Point::new(42, 224), large).draw(display)?;

    Rectangle::new(Point::new(22, 306), Size::new(436, 160))
        .into_styled(outline)
        .draw(display)?;
    Text::new("Relative humidity", Point::new(42, 348), heading).draw(display)?;
    Text::new(&humidity, Point::new(42, 420), large).draw(display)?;

    draw_action(display, 594, "Sensor details", body)?;
    draw_footer(display, state.display, "SELECT DETAILS  BOOT BACK")?;
    Ok(())
}

pub fn render_environment_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let id = state
        .board
        .environment_sensor_id
        .map_or_else(|| "Unavailable".into(), |id| format!("0x{id:04X}"));

    draw_header(display, state, "SENSOR DETAILS")?;

    Text::new("Sensor", Point::new(22, 120), heading).draw(display)?;
    line(display, 168, "Device ID", &id, body)?;
    line(display, 208, "Command", "Wake / measure / sleep", body)?;
    line(display, 248, "Validation", "Sensirion CRC-8", body)?;

    Text::new("Calibration", Point::new(22, 324), heading).draw(display)?;
    line(display, 372, "Compensation", "-1.5 C / -2.7 F", body)?;
    line(display, 412, "Live refresh", "Every 30 seconds", body)?;

    Text::new(
        "Press BOOT to return to Environment.",
        Point::new(22, 580),
        body,
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
