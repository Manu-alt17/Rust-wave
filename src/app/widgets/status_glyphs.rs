//! Persistent status glyphs (time, Wi-Fi, battery) shown in every header.
//!
//! Battery and Wi-Fi state used to appear only on Home, in oversized text.
//! This widget draws a small right-aligned summary that every header can
//! reuse so the information stays in view across the whole product shell.

use core::convert::Infallible;

use embedded_graphics::{
    image::Image,
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};
use embedded_iconoir::{
    icons::size24px::connectivity::{Wifi, WifiOff},
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        display::DisplayPreferences,
        typography::{Text, UiTextRole},
    },
    orientation::OrientedFrameBuffer,
};

/// Battery icon bounding box, `width x height` in logical pixels.
pub(crate) const BATTERY_SIZE: Size = Size::new(20, 12);
/// Extra width for the battery's positive-terminal nub.
const BATTERY_NUB_WIDTH: i32 = 2;
/// Wi-Fi icon bounding box, `width x height` in logical pixels. Matches the
/// `size24px` iconoir glyph exactly.
const WIFI_SIZE: Size = Size::new(24, 24);
const ICON_TEXT_GAP: i32 = 6;
const ICON_GROUP_GAP: i32 = 12;

/// Values shown by [`draw_persistent_status`]. Callers assemble this from
/// [`crate::app::state::AppState`] so this widget stays decoupled from the
/// application state type.
#[derive(Clone, Copy, Debug)]
pub struct PersistentStatus<'a> {
    pub date: &'a str,
    pub time: &'a str,
    pub wifi_connected: bool,
    pub battery_percent: Option<u8>,
}

/// Draw the persistent status line shared by every screen: `date` starting
/// at `anchor_left`, then `time`, a Wi-Fi icon and a battery icon/percentage
/// right-aligned so `anchor_right.x` is the rightmost inked pixel. Both
/// groups share the `y` baseline from `anchor_left`/`anchor_right`.
pub fn draw_persistent_status(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    anchor_left: Point,
    anchor_right: Point,
    status: PersistentStatus<'_>,
    color: BinaryColor,
) -> Result<(), Infallible> {
    let style = preferences.text_style(UiTextRole::Body, color);
    // Icons share one bottom edge (a few pixels below the text baseline, to
    // match a digit's descent) so unequal icon heights still line up instead
    // of drifting apart when they share a single fixed top instead.
    let icon_bottom = anchor_right.y + 3;

    Text::new(status.date, anchor_left, style).draw(display)?;

    let battery_label = status
        .battery_percent
        .map_or_else(|| "--".to_string(), |percent| format!("{percent}%"));
    let mut cursor_x = anchor_right.x - style.text_width(&battery_label);
    Text::new(&battery_label, Point::new(cursor_x, anchor_right.y), style).draw(display)?;

    cursor_x -= ICON_TEXT_GAP + BATTERY_SIZE.width as i32;
    draw_battery_icon(
        display,
        Point::new(cursor_x, icon_bottom - BATTERY_SIZE.height as i32),
        status.battery_percent,
        color,
    )?;

    cursor_x -= ICON_GROUP_GAP + WIFI_SIZE.width as i32;
    draw_wifi_icon(
        display,
        Point::new(cursor_x, icon_bottom - WIFI_SIZE.height as i32),
        status.wifi_connected,
        color,
    )?;

    cursor_x -= ICON_GROUP_GAP + style.text_width(status.time);
    Text::new(status.time, Point::new(cursor_x, anchor_right.y), style).draw(display)?;
    Ok(())
}

/// Draw a battery outline with a positive-terminal nub and a fill level
/// proportional to `percent`. An unknown percent draws an empty outline; the
/// caller pairs this with a `"--"` label.
pub(crate) fn draw_battery_icon(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    percent: Option<u8>,
    color: BinaryColor,
) -> Result<(), Infallible> {
    let outline = PrimitiveStyle::with_stroke(color, 1);
    let fill = PrimitiveStyle::with_fill(color);
    let body_width = BATTERY_SIZE.width - BATTERY_NUB_WIDTH as u32;

    Rectangle::new(top_left, Size::new(body_width, BATTERY_SIZE.height))
        .into_styled(outline)
        .draw(display)?;
    Rectangle::new(
        Point::new(top_left.x + body_width as i32, top_left.y + 3),
        Size::new(BATTERY_NUB_WIDTH as u32, 4),
    )
    .into_styled(fill)
    .draw(display)?;

    if let Some(percent) = percent {
        let usable_width = body_width.saturating_sub(4);
        let fill_width = usable_width * u32::from(percent.min(100)) / 100;
        if fill_width > 0 {
            Rectangle::new(
                Point::new(top_left.x + 2, top_left.y + 2),
                Size::new(fill_width, BATTERY_SIZE.height - 4),
            )
            .into_styled(fill)
            .draw(display)?;
        }
    }
    Ok(())
}

/// Draw the `embedded-iconoir` Wi-Fi glyph: the connected arcs, or the
/// dedicated crossed-out `WifiOff` glyph when disconnected.
fn draw_wifi_icon(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    connected: bool,
    color: BinaryColor,
) -> Result<(), Infallible> {
    if connected {
        Image::new(&Wifi::new(color), top_left).draw(display)
    } else {
        Image::new(&WifiOff::new(color), top_left).draw(display)
    }
}
