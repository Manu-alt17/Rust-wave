//! Read-only firmware, board and runtime information split across readable pages.

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
    build_info::{FIRMWARE_VERSION, PRODUCT_NAME},
    orientation::OrientedFrameBuffer,
};

/// Page 1/3: product firmware and display contract.
pub fn render_device_info(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let partials = format!("{} / 24", state.partial_refreshes);

    draw_header(display, state, "DEVICE INFO")?;

    Text::new("Firmware", Point::new(22, 118), heading).draw(display)?;
    line(display, 166, "Product", PRODUCT_NAME, body)?;
    line(display, 206, "Version", FIRMWARE_VERSION, body)?;
    line(display, 246, "Milestone", "Readability repair", body)?;

    Text::new("Display", Point::new(22, 326), heading).draw(display)?;
    line(display, 374, "Logical UI", "480 x 800 portrait", body)?;
    line(display, 414, "Native panel", "800 x 480 mono", body)?;
    line(display, 454, "Framebuffer", "48,000 bytes / 1-bpp", body)?;
    line(display, 494, "Partial chain", &partials, body)?;

    draw_action(display, 594, "Board services", body)?;
    draw_footer(display, state.display, "SELECT NEXT  BOOT BACK")?;
    Ok(())
}

/// Page 2/3: onboard services and read-only storage contract.
pub fn render_device_info_board(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    draw_header(display, state, "DEVICE INFO")?;

    Text::new("SDMMC storage", Point::new(22, 118), heading).draw(display)?;
    line(display, 166, "Mount", state.storage.status_label(), body)?;
    line(display, 206, "Mode", "4-bit FAT / read-only UI", body)?;
    Text::new("Pins", Point::new(22, 246), body).draw(display)?;
    Text::new(
        "CLK16 CMD17 D0=15 D1=7 D2=8 D3=18",
        Point::new(22, 280),
        detail,
    )
    .draw(display)?;

    draw_action(display, 348, "Runtime services", body)?;
    draw_footer(display, state.display, "SELECT NEXT  BOOT BACK")?;
    Ok(())
}

/// Page 3/3: network status and stable hardware ownership.
pub fn render_device_info_runtime(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let timezone = state.regional.timezone_label_for_rtc(state.board.rtc);

    draw_header(display, state, "DEVICE INFO")?;

    Text::new("Runtime services", Point::new(22, 118), heading).draw(display)?;
    line(display, 166, "Network", state.network.home_badge(), body)?;
    line(display, 206, "Weather", state.weather.home_badge(), body)?;
    line(display, 246, "RTC alarms", state.alarms.home_badge(), body)?;
    line(display, 286, "Display zone", &timezone, body)?;
    line(
        display,
        326,
        "Temperature",
        state.regional.temperature_unit.marker(),
        body,
    )?;

    Text::new("Stable ownership", Point::new(22, 404), heading).draw(display)?;
    line(display, 452, "EPD busy", "GPIO3 / ALDO3 managed", body)?;
    line(display, 492, "Buttons", "UP4 SELECT5 DOWN6", body)?;
    line(display, 532, "Power key", "Short menu / hold sleep", body)?;
    line(display, 572, "RTC alarm", "GPIO45 active-low", body)?;
    Text::new(
        "Press BOOT to return to page 2.",
        Point::new(22, 634),
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
    Text::new(value, Point::new(194, y), style).draw(display)?;
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
    use super::{render_device_info, render_device_info_board, render_device_info_runtime};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn device_info_pages_render_without_optional_services() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_device_info(&mut display, &state).unwrap();
        render_device_info_board(&mut display, &state).unwrap();
        render_device_info_runtime(&mut display, &state).unwrap();
    }
}
