//! Reusable black product header.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextRole},
        widgets::status_glyphs::{draw_persistent_status, PersistentStatus},
    },
    orientation::OrientedFrameBuffer,
};

/// Height of the header: the persistent status row plus the title row below
/// it. Shorter than the old two-line header now that the subtitle row is
/// gone, so every screen gains the reclaimed space back for its own content.
pub const HEADER_TOTAL_HEIGHT: u32 = 66;

/// Draw the product header shared by every portrait screen: a white bar with
/// the persistent date / time / Wi-Fi / battery status on top in black ink,
/// a divider line (matching the footer's separator style), and the compact
/// screen title below it.
pub fn draw_header(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    title: &str,
) -> Result<(), Infallible> {
    let preferences = state.display;
    let line = PrimitiveStyle::with_fill(BinaryColor::On);
    let title_style = preferences.text_style(UiTextRole::Heading, BinaryColor::On);

    draw_persistent_status(
        display,
        preferences,
        Point::new(18, 21),
        Point::new(462, 21),
        PersistentStatus {
            date: &state.status_date_label(),
            time: &state.status_time_label(),
            wifi_connected: state.wifi_connected(),
            battery_percent: state.battery_percent(),
        },
        BinaryColor::On,
    )?;
    Rectangle::new(Point::new(14, 33), Size::new(452, 1))
        .into_styled(line)
        .draw(display)?;
    Text::new(title, Point::new(18, 56), title_style).draw(display)?;
    Ok(())
}
