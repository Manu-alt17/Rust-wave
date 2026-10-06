//! The device and its keys, drawn: where each key is and how it moves.
//!
//! The first-run page about the keys shows the device as it is held, upright,
//! with the rocker on its left edge near the top and the two keys on the
//! right edge, Power above BOOT, each named beside it; small pictures of the
//! gestures (turn, press, hold) then go with the lines that say what each
//! one does.
//!
//! The rocker is a ridged wheel of which half shows beyond the edge, and
//! that is how it is drawn, in the figure and in the gestures alike: a
//! half disc with notches around its rim. Drawn as a block it read as one
//! more key to press.
//!
//! The proportions are those of the board (303 x 505, measured on its
//! picture): the rocker a fifth of the way down the left edge, Power and
//! BOOT at 15% and 25% of the right one. The keys are drawn larger than
//! they are, or at this size they would be a couple of pixels.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{
        Circle, CornerRadii, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle,
        RoundedRectangle, Triangle,
    },
};

use crate::{
    app::typography::{Text, UiTextStyle},
    orientation::OrientedFrameBuffer,
};

/// Width of the device over its height, in thousandths.
const BODY_ASPECT: i32 = 600;
/// How far down the left edge the rocker's center is, in thousandths.
const ROCKER_AT: i32 = 206;
/// How far down the right edge the two keys are, in thousandths.
const POWER_AT: i32 = 154;
const BOOT_AT: i32 = 255;
/// Stroke of the device's outline and of the lines to the names.
const OUTLINE: u32 = 3;
const LEADER: u32 = 2;
/// Gap between a key and the start of its line, and between the line's end
/// and the name.
const LEADER_GAP: i32 = 5;
/// Length of the lines to the names.
const LEADER_LENGTH: i32 = 30;

/// Width of the column the gesture pictures are drawn in.
pub const GESTURE_ICON_WIDTH: i32 = 56;
/// Height the gesture pictures need.
pub const GESTURE_ICON_HEIGHT: i32 = 30;
/// Radius of the rocker's wheel in the gesture pictures.
const GESTURE_WHEEL_RADIUS: i32 = 13;
/// Where the grip's notches are around the wheel's rim, as (x, y) in
/// thousandths: straight out of the edge, then every 25 degrees either way.
const WHEEL_NOTCHES: [(i32, i32); 7] = [
    (-1000, 0),
    (-906, -423),
    (-906, 423),
    (-643, -766),
    (-643, 766),
    (-259, -966),
    (-259, 966),
];

/// The names written beside the keys, in the user's language.
#[derive(Clone, Copy, Debug)]
pub struct KeyNames<'a> {
    pub rocker: &'a str,
    pub power: &'a str,
    pub boot: &'a str,
}

/// One of the two keys on the right edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EdgeKey {
    Power,
    Boot,
}

fn ink() -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_fill(BinaryColor::On)
}

fn stroke(width: u32) -> PrimitiveStyle<BinaryColor> {
    PrimitiveStyle::with_stroke(BinaryColor::On, width)
}

/// Radius of the rocker's wheel in a figure `height` tall.
#[must_use]
pub const fn device_figure_wheel_radius(height: i32) -> i32 {
    let radius = height * 58 / 1000;
    if radius < 12 {
        12
    } else {
        radius
    }
}

/// The rocker as it shows from the front: the half of a ridged wheel that
/// comes out of the device's left edge at `edge_x`, centered on `center_y`.
fn draw_wheel(
    display: &mut OrientedFrameBuffer<'_>,
    edge_x: i32,
    center_y: i32,
    radius: i32,
) -> Result<(), Infallible> {
    // The half disc, a row at a time.
    for dy in -radius..=radius {
        let mut reach = 0;
        while (reach + 1) * (reach + 1) + dy * dy <= radius * radius + radius / 2 {
            reach += 1;
        }
        if reach > 0 {
            Rectangle::new(
                Point::new(edge_x - reach, center_y + dy),
                Size::new(reach as u32, 1),
            )
            .into_styled(ink())
            .draw(display)?;
        }
    }
    // The grip: small notches bitten out of the rim, the disc left whole.
    for (x, y) in WHEEL_NOTCHES {
        Rectangle::new(
            Point::new(
                edge_x + x * (radius - 1) / 1000 - 1,
                center_y + y * (radius - 1) / 1000 - 1,
            ),
            Size::new(2, 2),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
        .draw(display)?;
    }
    Ok(())
}

/// Width of a figure `height` tall, without the names beside it.
#[must_use]
pub const fn device_figure_width(height: i32) -> i32 {
    height * BODY_ASPECT / 1000
}

/// The device upright, `height` tall with its body centered on `center_x`,
/// and the three names: the rocker's on the left with the two arrows of its
/// travel, Power's and BOOT's on the right.
pub fn draw_device_figure(
    display: &mut OrientedFrameBuffer<'_>,
    center_x: i32,
    top: i32,
    height: i32,
    names: KeyNames<'_>,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let width = device_figure_width(height);
    let left = center_x - width / 2;
    let right = left + width;
    let bottom = top + height;

    // Keys first: the body's outline then closes over where they meet it.
    let rocker_y = top + height * ROCKER_AT / 1000;
    let wheel = device_figure_wheel_radius(height);
    draw_wheel(display, left + 1, rocker_y, wheel)?;
    let key_half = (height * 30 / 1000).max(6);
    let key_out = (width * 45 / 1000).max(5);
    let power_y = top + height * POWER_AT / 1000;
    let boot_y = top + height * BOOT_AT / 1000;
    for key_y in [power_y, boot_y] {
        Rectangle::new(
            Point::new(right - 4, key_y - key_half),
            Size::new((key_out + 4) as u32, (2 * key_half) as u32),
        )
        .into_styled(ink())
        .draw(display)?;
    }

    // Body and glass: the glass leaves a wider band at the bottom, where
    // the panel's cable is folded.
    let radius = (height / 24).max(6) as u32;
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(left, top),
            Size::new(width as u32, height as u32),
        ),
        CornerRadii::new(Size::new(radius, radius)),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .stroke_color(BinaryColor::On)
            .stroke_width(OUTLINE)
            .fill_color(BinaryColor::Off)
            .build(),
    )
    .draw(display)?;
    let side = (width * 60 / 1000).max(6);
    let glass_top = top + side;
    let glass_bottom = bottom - (height * 90 / 1000).max(side + 6);
    Rectangle::new(
        Point::new(left + side, glass_top),
        Size::new((width - 2 * side) as u32, (glass_bottom - glass_top) as u32),
    )
    .into_styled(stroke(1))
    .draw(display)?;
    // A page of text on the glass: a heading and lines, the last one short.
    let text_left = left + side + (width * 90 / 1000).max(8);
    let text_width = width - 2 * (text_left - left);
    let step = ((glass_bottom - glass_top) / 11).max(8);
    for line in 0..8 {
        let y = glass_top + step * 2 + line * step;
        if y + 2 >= glass_bottom - step / 2 {
            break;
        }
        let (length, thickness) = match line {
            0 => (text_width * 6 / 10, 3),
            1 => continue,
            7 => (text_width * 5 / 10, 1),
            _ => (text_width, 1),
        };
        Rectangle::new(
            Point::new(text_left, y),
            Size::new(length as u32, thickness),
        )
        .into_styled(ink())
        .draw(display)?;
    }

    // The rocker's travel, up and down, and its name.
    let arrow_x = left - wheel / 2 - 2;
    draw_vertical_arrowhead(display, arrow_x, rocker_y - wheel - 13, true)?;
    draw_vertical_arrowhead(display, arrow_x, rocker_y + wheel + 13, false)?;
    let (cap_top, _) = style.text_ink_bounds("H");
    let line_end = left - wheel - LEADER_GAP;
    let line_start = line_end - LEADER_LENGTH;
    Line::new(
        Point::new(line_start, rocker_y),
        Point::new(line_end, rocker_y),
    )
    .into_styled(stroke(LEADER))
    .draw(display)?;
    Text::new(
        names.rocker,
        Point::new(
            line_start - LEADER_GAP - style.text_width(names.rocker),
            rocker_y - cap_top / 2,
        ),
        style,
    )
    .draw(display)?;

    // The two keys can be closer than two names fit one above the other:
    // the names are then set apart, by enough for the bend in their lines
    // to read as one, and the lines reach them at 45 degrees.
    let needed = -cap_top + 6;
    let apart = boot_y - power_y;
    let spread = if apart >= needed {
        0
    } else {
        ((needed - apart + 1) / 2).max(6)
    };
    let key_end = right + key_out + LEADER_GAP;
    let bend = key_end + 6;
    let knee = bend + spread;
    let line_end = (key_end + LEADER_LENGTH).max(knee + 8);
    for (name, key_y, name_y) in [
        (names.power, power_y, power_y - spread),
        (names.boot, boot_y, boot_y + spread),
    ] {
        for (from, to) in [
            (Point::new(key_end, key_y), Point::new(bend, key_y)),
            (Point::new(bend, key_y), Point::new(knee, name_y)),
            (Point::new(knee, name_y), Point::new(line_end, name_y)),
        ] {
            Line::new(from, to)
                .into_styled(stroke(LEADER))
                .draw(display)?;
        }
        Text::new(
            name,
            Point::new(line_end + LEADER_GAP, name_y - cap_top / 2),
            style,
        )
        .draw(display)?;
    }
    Ok(())
}

/// A filled arrowhead centered on `center_x`, its tip `tip_y`: pointing up
/// or down.
fn draw_vertical_arrowhead(
    display: &mut OrientedFrameBuffer<'_>,
    center_x: i32,
    tip_y: i32,
    up: bool,
) -> Result<(), Infallible> {
    let base_y = if up { tip_y + 8 } else { tip_y - 8 };
    Triangle::new(
        Point::new(center_x - 7, base_y),
        Point::new(center_x + 7, base_y),
        Point::new(center_x, tip_y),
    )
    .into_styled(ink())
    .draw(display)
}

/// An arrow along a row, from `from_x` to its tip at `tip_x`, whichever
/// side that is.
fn draw_horizontal_arrow(
    display: &mut OrientedFrameBuffer<'_>,
    from_x: i32,
    tip_x: i32,
    center_y: i32,
) -> Result<(), Infallible> {
    let towards_right = tip_x > from_x;
    let head = 8;
    let base_x = if towards_right {
        tip_x - head
    } else {
        tip_x + head
    };
    let (shaft_left, shaft_right) = if towards_right {
        (from_x, base_x)
    } else {
        (base_x, from_x)
    };
    Rectangle::new(
        Point::new(shaft_left, center_y - 1),
        Size::new((shaft_right - shaft_left + 1).max(1) as u32, 3),
    )
    .into_styled(ink())
    .draw(display)?;
    Triangle::new(
        Point::new(base_x, center_y - 6),
        Point::new(base_x, center_y + 6),
        Point::new(tip_x, center_y),
    )
    .into_styled(ink())
    .draw(display)
}

/// The rocker as the gesture pictures show it: a piece of the left edge
/// with the wheel on it, centered on `center_y`. Returns the x of the
/// wheel's outer side, where a press arrow ends.
fn draw_rocker_piece(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    center_y: i32,
) -> Result<i32, Infallible> {
    let edge_x = left + GESTURE_ICON_WIDTH - 6;
    draw_wheel(display, edge_x + 1, center_y, GESTURE_WHEEL_RADIUS)?;
    Rectangle::new(
        Point::new(edge_x, center_y - GESTURE_ICON_HEIGHT / 2),
        Size::new(OUTLINE, GESTURE_ICON_HEIGHT as u32),
    )
    .into_styled(ink())
    .draw(display)?;
    Ok(edge_x - GESTURE_WHEEL_RADIUS)
}

/// Turning the rocker: an arrow up and one down beside the wheel.
pub fn draw_turn_gesture(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    center_y: i32,
) -> Result<(), Infallible> {
    let wheel_left = draw_rocker_piece(display, left, center_y)?;
    let arrow_x = wheel_left - 11;
    for (tip_y, tail_y, up) in [
        (center_y - 14, center_y - 3, true),
        (center_y + 14, center_y + 3, false),
    ] {
        draw_vertical_arrowhead(display, arrow_x, tip_y, up)?;
        let (shaft_top, shaft_bottom) = if up {
            (tip_y + 8, tail_y)
        } else {
            (tail_y, tip_y - 8)
        };
        Rectangle::new(
            Point::new(arrow_x - 1, shaft_top),
            Size::new(3, (shaft_bottom - shaft_top + 1).max(1) as u32),
        )
        .into_styled(ink())
        .draw(display)?;
    }
    Ok(())
}

/// Pressing the rocker: an arrow into the wheel. Held down, a clock before
/// the arrow.
pub fn draw_press_gesture(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    center_y: i32,
    held: bool,
) -> Result<(), Infallible> {
    let wheel_left = draw_rocker_piece(display, left, center_y)?;
    let tip_x = wheel_left - 4;
    if held {
        draw_clock(display, left + 9, center_y)?;
        draw_horizontal_arrow(display, left + 21, tip_x, center_y)
    } else {
        draw_horizontal_arrow(display, left + 6, tip_x, center_y)
    }
}

/// A small clock face: "for a while".
fn draw_clock(
    display: &mut OrientedFrameBuffer<'_>,
    center_x: i32,
    center_y: i32,
) -> Result<(), Infallible> {
    Circle::new(Point::new(center_x - 8, center_y - 8), 17)
        .into_styled(stroke(2))
        .draw(display)?;
    Line::new(
        Point::new(center_x, center_y),
        Point::new(center_x, center_y - 5),
    )
    .into_styled(stroke(2))
    .draw(display)?;
    Line::new(
        Point::new(center_x, center_y),
        Point::new(center_x + 4, center_y),
    )
    .into_styled(stroke(2))
    .draw(display)
}

/// One of the keys on the right edge, pressed: a piece of that edge with
/// both keys, the one meant filled and the other only outlined, and an
/// arrow into the filled one. Held down, a clock after the arrow. The
/// mirror image of the rocker's pictures, as the right edge is of the left.
pub fn draw_edge_key_gesture(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    center_y: i32,
    key: EdgeKey,
    held: bool,
) -> Result<(), Infallible> {
    let edge_x = left + 3;
    Rectangle::new(
        Point::new(edge_x, center_y - GESTURE_ICON_HEIGHT / 2),
        Size::new(OUTLINE, GESTURE_ICON_HEIGHT as u32),
    )
    .into_styled(ink())
    .draw(display)?;
    let key_left = edge_x + OUTLINE as i32 - 1;
    let mut pressed_y = center_y;
    for (which, key_y) in [
        (EdgeKey::Power, center_y - 6),
        (EdgeKey::Boot, center_y + 6),
    ] {
        let shape = Rectangle::new(Point::new(key_left, key_y - 4), Size::new(9, 9));
        if which == key {
            pressed_y = key_y;
            shape.into_styled(ink()).draw(display)?;
        } else {
            shape.into_styled(stroke(1)).draw(display)?;
        }
    }
    let tip_x = key_left + 9 + 4;
    let right = left + GESTURE_ICON_WIDTH;
    if held {
        draw_clock(display, right - 10, pressed_y)?;
        draw_horizontal_arrow(display, right - 22, tip_x, pressed_y)
    } else {
        draw_horizontal_arrow(display, right - 7, tip_x, pressed_y)
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};

    use super::{
        device_figure_wheel_radius, device_figure_width, draw_device_figure, draw_edge_key_gesture,
        draw_press_gesture, draw_turn_gesture, EdgeKey, KeyNames, BOOT_AT, GESTURE_ICON_HEIGHT,
        GESTURE_ICON_WIDTH, GESTURE_WHEEL_RADIUS, POWER_AT, ROCKER_AT,
    };
    use crate::{
        app::{
            display::UiFontSize,
            typography::{audit, style_for, UiTextRole},
        },
        framebuffer::FrameBuffer,
        orientation::{DisplayOrientation, OrientedFrameBuffer},
    };

    const NAMES: KeyNames<'static> = KeyNames {
        rocker: "Rotella",
        power: "Power",
        boot: "BOOT",
    };

    fn black(frame: &FrameBuffer, x: i32, y: i32) -> bool {
        DisplayOrientation::Portrait
            .map_logical_to_native(Point::new(x, y))
            .and_then(|native| frame.is_black(native))
            .unwrap_or(false)
    }

    /// Bounding box of everything inked in `frame`.
    fn inked(frame: &FrameBuffer) -> (i32, i32, i32, i32) {
        let mut bounds = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..800 {
            for x in 0..480 {
                if black(frame, x, y) {
                    bounds = (
                        bounds.0.min(x),
                        bounds.1.min(y),
                        bounds.2.max(x),
                        bounds.3.max(y),
                    );
                }
            }
        }
        bounds
    }

    #[test]
    fn the_keys_are_where_they_are_on_the_device() {
        for (height, size) in [
            (150, UiFontSize::Large),
            (200, UiFontSize::Standard),
            (240, UiFontSize::Compact),
        ] {
            let style = style_for(size, UiTextRole::Heading, BinaryColor::On);
            let mut frame = FrameBuffer::new_white();
            let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
            audit::start();
            draw_device_figure(&mut display, 240, 100, height, NAMES, style).unwrap();
            let records = audit::take();
            let width = device_figure_width(height);
            let (left, right) = (240 - width / 2, 240 - width / 2 + width);

            // The rocker sticks out of the left edge a fifth of the way
            // down; nothing does on that edge lower than the keys.
            let rocker_y = 100 + height * ROCKER_AT / 1000;
            let wheel = device_figure_wheel_radius(height);
            assert!(black(&frame, left - 3, rocker_y + 2));
            assert!(!black(&frame, left - 4, 100 + height * 600 / 1000));
            // It is half a wheel, not a block: nothing in the corners of
            // the box around it.
            assert!(!black(&frame, left - wheel + 2, rocker_y - wheel + 2));
            assert!(!black(&frame, left - wheel + 2, rocker_y + wheel - 2));
            // And a ridged one: along its middle the rim is cut by a notch,
            // with a tooth of the grip on either side of it.
            let rim = left - wheel + 2;
            assert!(
                (-1..=1).any(|dy| !black(&frame, rim, rocker_y + dy)),
                "{height}: no notch"
            );
            let tooth = (
                left + 1 - (wheel - 1) * 976 / 1000,
                (wheel - 1) * 216 / 1000,
            );
            assert!(black(&frame, tooth.0, rocker_y + tooth.1), "{height}");
            assert!(black(&frame, tooth.0, rocker_y - tooth.1), "{height}");
            // Power above BOOT on the right edge, with a gap between them.
            let power_y = 100 + height * POWER_AT / 1000;
            let boot_y = 100 + height * BOOT_AT / 1000;
            assert!(black(&frame, right + 3, power_y));
            assert!(black(&frame, right + 3, boot_y));
            assert!(!black(&frame, right + 3, (power_y + boot_y) / 2));
            assert!(!black(&frame, right + 3, 100 + height * 600 / 1000));

            // The rocker's name is to the left of the device, the other
            // two to its right, Power's above BOOT's, and none over another
            // or off the screen.
            let named = |text: &str| records.iter().find(|rec| rec.text == text).unwrap();
            let (rocker, power, boot) = (named("Rotella"), named("Power"), named("BOOT"));
            assert!(rocker.x >= 0 && rocker.x + rocker.width < left, "{height}");
            assert!(power.x > right && boot.x > right);
            assert!(power.x + power.width <= 480 && boot.x + boot.width <= 480);
            assert!(power.ink_bottom < boot.ink_top, "{height}: names touch");
        }
    }

    #[test]
    fn the_gesture_pictures_stay_in_their_box() {
        let draws: [(&str, fn(&mut OrientedFrameBuffer<'_>)); 7] = [
            ("turn", |display| {
                draw_turn_gesture(display, 100, 200).unwrap();
            }),
            ("press", |display| {
                draw_press_gesture(display, 100, 200, false).unwrap();
            }),
            ("hold", |display| {
                draw_press_gesture(display, 100, 200, true).unwrap();
            }),
            ("power", |display| {
                draw_edge_key_gesture(display, 100, 200, EdgeKey::Power, false).unwrap();
            }),
            ("power-hold", |display| {
                draw_edge_key_gesture(display, 100, 200, EdgeKey::Power, true).unwrap();
            }),
            ("boot", |display| {
                draw_edge_key_gesture(display, 100, 200, EdgeKey::Boot, false).unwrap();
            }),
            ("boot-hold", |display| {
                draw_edge_key_gesture(display, 100, 200, EdgeKey::Boot, true).unwrap();
            }),
        ];
        let mut frames = Vec::new();
        for (name, draw) in draws {
            let mut frame = FrameBuffer::new_white();
            let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
            draw(&mut display);
            let (left, top, right, bottom) = inked(&frame);
            assert!(left >= 100 && right < 100 + GESTURE_ICON_WIDTH, "{name}");
            assert!(
                top >= 200 - GESTURE_ICON_HEIGHT / 2 && bottom <= 200 + GESTURE_ICON_HEIGHT / 2,
                "{name}: {top}..{bottom}"
            );
            frames.push(frame);
        }
        // The rocker's pictures show the wheel: rounded where a key would
        // have corners. Its edge is 6 px in from the right of the box.
        let wheel_left = 100 + GESTURE_ICON_WIDTH - 6 - GESTURE_WHEEL_RADIUS;
        for frame in &frames[..3] {
            assert!(black(frame, wheel_left + GESTURE_WHEEL_RADIUS - 2, 203));
            assert!(!black(
                frame,
                wheel_left + 2,
                200 - GESTURE_WHEEL_RADIUS + 2
            ));
            assert!(!black(
                frame,
                wheel_left + 2,
                200 + GESTURE_WHEEL_RADIUS - 2
            ));
        }
        // Each gesture has a picture of its own.
        for (index, frame) in frames.iter().enumerate() {
            for other in frames.iter().skip(index + 1) {
                assert_ne!(frame, other);
            }
        }
    }
}
