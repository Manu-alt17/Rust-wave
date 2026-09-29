//! Persistent status glyphs (Wi-Fi, battery) shown in every header.
//!
//! Battery and Wi-Fi state used to appear only on Home, in oversized text.
//! This widget draws a small left-aligned summary that every header can
//! reuse so the information stays in view across the whole product shell.

use core::convert::Infallible;

use embedded_graphics::{
    image::Image,
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Size},
};
use embedded_iconoir::{
    icons::size24px::{
        connectivity::{Wifi, WifiOff},
        photos_and_videos::Flash,
        system::{
            BatteryCharging, BatteryEmpty, BatteryFiveZero, BatteryFull, BatterySevenFive,
            BatteryTwoFive,
        },
    },
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        display::DisplayPreferences,
        typography::{Text, UiTextRole},
    },
    orientation::OrientedFrameBuffer,
};

/// Battery icon bounding box, `width x height` in logical pixels: a
/// `size24px` iconoir glyph. Used by the Reader footer, its only caller now
/// that the header shows the percentage as plain text instead.
pub(crate) const BATTERY_SIZE: Size = Size::new(24, 24);
/// Wi-Fi icon bounding box, `width x height` in logical pixels. Matches the
/// `size24px` iconoir glyph exactly.
const WIFI_SIZE: Size = Size::new(24, 24);
/// Charging-indicator icon bounding box. Also a `size24px` iconoir glyph, so
/// it shares `WIFI_SIZE`'s dimensions.
const CHARGING_ICON_SIZE: Size = WIFI_SIZE;
const ICON_TEXT_GAP: i32 = 6;

/// Text role shared by the header clock and the battery percentage. `Large`
/// (24px source size) read almost as tall as the 24px Wi-Fi icon itself —
/// no longer visibly smaller the way `Body` was — so it overshot; `Heading`
/// (20px source size) is one tier down, about 15% smaller, and still reads
/// clearly bigger than the shell's default `Body` text.
const STATUS_TEXT_ROLE: UiTextRole = UiTextRole::Heading;

/// Draw the persistent status glyphs shared by every header: a Wi-Fi icon,
/// then — while external power is connected, per `AppState::battery_charging`
/// — a small flash glyph, then the battery percentage (no battery-shaped
/// icon — text only). Left-aligned so `anchor_left.x` is the leftmost inked
/// pixel (the Wi-Fi icon's left edge) and `anchor_left.y` is the shared text
/// baseline. Returns the x just past the battery percentage, so the header
/// can keep its title clear of the group.
pub fn draw_status_group(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    anchor_left: Point,
    wifi_connected: bool,
    battery_percent: Option<u8>,
    charging: bool,
    color: BinaryColor,
) -> Result<i32, Infallible> {
    let style = preferences.text_style(STATUS_TEXT_ROLE, color);
    // The icon's bottom sits a few pixels below the text baseline, to match
    // a digit's descent, so it reads as vertically centered on the text.
    let icon_bottom = anchor_left.y + 3;

    draw_wifi_icon(
        display,
        Point::new(anchor_left.x, icon_bottom - WIFI_SIZE.height as i32),
        wifi_connected,
        color,
    )?;

    let mut text_x = anchor_left.x + WIFI_SIZE.width as i32 + ICON_TEXT_GAP;
    if charging {
        Image::new(
            &Flash::new(color),
            Point::new(text_x, icon_bottom - CHARGING_ICON_SIZE.height as i32),
        )
        .draw(display)?;
        text_x += CHARGING_ICON_SIZE.width as i32 + ICON_TEXT_GAP;
    }

    let battery_label =
        battery_percent.map_or_else(|| "--%".to_string(), |percent| format!("{percent}%"));
    let end = Text::new(&battery_label, Point::new(text_x, anchor_left.y), style).draw(display)?;
    Ok(end.x)
}

/// Draw the `embedded-iconoir` battery glyph for `percent`: the charging
/// glyph whenever `charging` (pass `AppState::battery_charging`, the same
/// signal that shows the header's flash glyph), otherwise the level glyph
/// the percentage rounds to -- 25 / 50 / 75 / full. An unknown percent draws
/// the empty glyph.
pub(crate) fn draw_battery_icon(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    percent: Option<u8>,
    charging: bool,
    color: BinaryColor,
) -> Result<(), Infallible> {
    if charging {
        return Image::new(&BatteryCharging::new(color), top_left).draw(display);
    }
    match percent {
        None => Image::new(&BatteryEmpty::new(color), top_left).draw(display),
        Some(0..=37) => Image::new(&BatteryTwoFive::new(color), top_left).draw(display),
        Some(38..=62) => Image::new(&BatteryFiveZero::new(color), top_left).draw(display),
        Some(63..=87) => Image::new(&BatterySevenFive::new(color), top_left).draw(display),
        Some(_) => Image::new(&BatteryFull::new(color), top_left).draw(display),
    }
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
