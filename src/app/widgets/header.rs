//! Reusable black product header.

use core::convert::Infallible;

use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextRole},
        widgets::status_glyphs::draw_status_group,
    },
    orientation::OrientedFrameBuffer,
};

/// Height of the header: a single row holding the persistent Wi-Fi /
/// battery status on the left, the screen title centered, and the clock on
/// the right, all on one baseline. No divider line below it.
pub const HEADER_TOTAL_HEIGHT: u32 = 44;

/// Left margin of the Wi-Fi / battery status group.
const STATUS_LEFT: i32 = 18;
/// Rightmost inked pixel of the clock.
const TIME_RIGHT: i32 = 462;
/// Logical portrait screen width, used to center the title between the
/// status group and the clock.
const SCREEN_WIDTH: i32 = 480;

/// Draw the product header shared by every portrait screen: the persistent
/// Wi-Fi / battery status on the left, the screen title centered, and the
/// clock on the right, all on one baseline, in black ink on the white
/// background.
pub fn draw_header(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    title: &str,
) -> Result<(), Infallible> {
    let preferences = state.display;
    let title_style = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
    let baseline_y = 30;

    draw_status_group(
        display,
        preferences,
        Point::new(STATUS_LEFT, baseline_y),
        state.wifi_connected(),
        state.battery_percent(),
        state.battery_charging(),
        BinaryColor::On,
    )?;

    let title_x = (SCREEN_WIDTH - title_style.text_width(title)) / 2;
    Text::new(title, Point::new(title_x, baseline_y), title_style).draw(display)?;

    let time_label = state.status_time_label();
    let time_x = TIME_RIGHT - title_style.text_width(&time_label);
    Text::new(&time_label, Point::new(time_x, baseline_y), title_style).draw(display)?;
    Ok(())
}
