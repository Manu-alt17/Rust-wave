//! QMI8658-backed raw motion diagnostics and native event-bridge sample screen.

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
    imu::{format_tenths, Axis3Tenths},
    imu_events::{ImuEventBridge, IMU_EVENT_CONTROL_COUNT},
    orientation::OrientedFrameBuffer,
};

/// Draw QMI8658 accelerometer and gyroscope readings.
pub fn render_motion(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let outline = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    draw_header(display, state, "MOTION")?;

    Text::new("Accelerometer", Point::new(22, 108), heading).draw(display)?;
    Rectangle::new(Point::new(22, 138), Size::new(436, 144))
        .into_styled(outline)
        .draw(display)?;
    if let Some(reading) = state.board.imu {
        draw_axis_lines(display, 180, reading.acceleration_mg_tenths, "mg", body)?;
    } else {
        Text::new("QMI8658 unavailable", Point::new(42, 206), body).draw(display)?;
    }

    Text::new("Gyroscope", Point::new(22, 326), heading).draw(display)?;
    Rectangle::new(Point::new(22, 356), Size::new(436, 144))
        .into_styled(outline)
        .draw(display)?;
    if let Some(reading) = state.board.imu {
        draw_axis_lines(display, 398, reading.gyroscope_dps_tenths, "dps", body)?;
    } else {
        Text::new("QMI8658 unavailable", Point::new(42, 424), body).draw(display)?;
    }

    draw_action(display, 594, "Motion event bridge", body)?;
    draw_footer(display, state.display, "SELECT EVENTS  BOOT BACK")?;
    Ok(())
}

/// Draw debounced tilt, shake, rotate and level events plus threshold controls.
pub fn render_motion_events(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let latest = state.imu_events.latest_label();

    draw_header(display, state, "MOTION EVENTS")?;

    Text::new("Native event diagnostics", Point::new(22, 100), heading).draw(display)?;
    line(display, 146, "Latest", &latest, body)?;
    line(
        display,
        184,
        "Counts",
        &format!(
            "T{} S{} R{} L{}",
            state.imu_events.counters.tilt,
            state.imu_events.counters.shake,
            state.imu_events.counters.rotate,
            state.imu_events.counters.level
        ),
        body,
    )?;
    Text::new(
        "Raw QMI8658 stays behind Rust I2C.",
        Point::new(22, 224),
        detail,
    )
    .draw(display)?;

    Text::new("Thresholds and debounce", Point::new(22, 276), heading).draw(display)?;
    for index in 0..IMU_EVENT_CONTROL_COUNT {
        draw_control(
            display,
            &state.imu_events,
            index,
            308 + index as i32 * 42,
            body,
        )?;
    }
    draw_footer(
        display,
        state.display,
        "UP/DOWN ROW SELECT CHANGE BOOT BACK",
    )?;
    Ok(())
}

pub fn render_motion_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    draw_header(display, state, "MOTION DETAILS")?;

    Text::new("Sensor status", Point::new(22, 118), heading).draw(display)?;
    if let Some(reading) = state.board.imu {
        let address = state
            .board
            .imu_address
            .map_or_else(|| "--".into(), |value| format!("0x{value:02X}"));
        let revision = state
            .board
            .imu_revision
            .map_or_else(|| "--".into(), |value| format!("0x{value:02X}"));
        line(display, 168, "Magnitude", &reading.magnitude_label(), body)?;
        line(
            display,
            208,
            "Dominant axis",
            reading.dominant_axis.label(),
            body,
        )?;
        line(
            display,
            248,
            "Address / rev",
            &format!("{address} / {revision}"),
            body,
        )?;
        line(
            display,
            288,
            "Die temperature",
            &reading.temperature_label(),
            body,
        )?;
        line(
            display,
            328,
            "STATUS0",
            &format!("0x{:02X}", reading.status0),
            body,
        )?;
    } else {
        Text::new(
            "Optional IMU service unavailable.",
            Point::new(22, 168),
            body,
        )
        .draw(display)?;
    }

    Text::new("Profile", Point::new(22, 416), heading).draw(display)?;
    Text::new("+/-8 g and +/-512 dps", Point::new(22, 464), body).draw(display)?;
    Text::new(
        "Sample rate: 1000 Hz sensor / 80 ms bridge",
        Point::new(22, 504),
        body,
    )
    .draw(display)?;
    Text::new(
        "Technical tokens remain compact.",
        Point::new(22, 566),
        detail,
    )
    .draw(display)?;
    draw_footer(display, state.display, "BOOT BACK")?;
    Ok(())
}

fn draw_control(
    display: &mut OrientedFrameBuffer<'_>,
    bridge: &ImuEventBridge,
    index: usize,
    y: i32,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    if index == bridge.selected_control {
        Rectangle::new(Point::new(18, y - 27), Size::new(444, 36))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 3))
            .draw(display)?;
        Text::new(">", Point::new(28, y), style).draw(display)?;
    }
    Text::new(
        ImuEventBridge::control_label(index),
        Point::new(52, y),
        style,
    )
    .draw(display)?;
    Text::new(&bridge.control_value(index), Point::new(306, y), style).draw(display)?;
    Ok(())
}

fn draw_axis_lines(
    display: &mut OrientedFrameBuffer<'_>,
    start_y: i32,
    axes: Axis3Tenths,
    unit: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(
        &format!("X axis      {} {unit}", format_tenths(axes.x)),
        Point::new(42, start_y),
        style,
    )
    .draw(display)?;
    Text::new(
        &format!("Y axis      {} {unit}", format_tenths(axes.y)),
        Point::new(42, start_y + 38),
        style,
    )
    .draw(display)?;
    Text::new(
        &format!("Z axis      {} {unit}", format_tenths(axes.z)),
        Point::new(42, start_y + 76),
        style,
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
    Text::new(value, Point::new(202, y), style).draw(display)?;
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
    use super::{render_motion, render_motion_details, render_motion_events};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn motion_overview_events_and_details_render_without_imu() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_motion(&mut display, &state).unwrap();
        render_motion_events(&mut display, &state).unwrap();
        render_motion_details(&mut display, &state).unwrap();
    }
}
