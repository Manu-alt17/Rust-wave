//! Book-cover sleep screen: the cover of the book being read, full-screen,
//! with a white rounded "resume" tab: reading progress, "Riprendi" and an
//! arrow.
//!
//! The tab is anchored to the right edge and runs off it, so only its left
//! corners are rounded and it reads as cut by the bezel. It sits at a fixed
//! height, [`RESUME_TAB_BAND`], level with the physical power key that wakes
//! the device, so its arrow points at the key. The percent uses the largest
//! UI strike that still fits that band.
//!
//! Drawn through the same portrait mapping as every product screen, since
//! sleep frames are native-panel buffers.

use core::convert::Infallible;

use embedded_graphics::{
    image::{Image, ImageRaw},
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle, Triangle},
};

use crate::{
    app::{
        display::UiFontSize,
        i18n::t,
        typography::{style_for, Text, UiTextRole, UiTextStyle},
    },
    cover_cache::CachedThumbnail,
    framebuffer::FrameBuffer,
    orientation::{DisplayOrientation, OrientedFrameBuffer},
    regional::Locale,
};

/// Portrait logical y-range of the resume tab: level with the physical power
/// key on the right edge. It matches the bottom of the first to the bottom of
/// the third body line of a Reader page at default preferences, but is a
/// hardware position and must not follow Reader font changes.
pub const RESUME_TAB_BAND: (i32, i32) = (69, 139);

/// Full-screen cover size: the portrait logical surface.
pub const SLEEP_COVER_WIDTH: u16 = DisplayOrientation::Portrait.logical_size().width as u16;
pub const SLEEP_COVER_HEIGHT: u16 = DisplayOrientation::Portrait.logical_size().height as u16;

const SCREEN_WIDTH: i32 = SLEEP_COVER_WIDTH as i32;
const TAB_PAD_X: i32 = 20;
const TAB_PAD_Y: i32 = 5;
const TAB_STROKE: u32 = 3;
const TAB_MAX_RADIUS: u32 = 28;
const LABEL_GAP: i32 = 10;
const ARROW_GAP: i32 = 8;
const ARROW_SHAFT: i32 = 3;

/// Percent strikes, largest first.
const PERCENT_STYLES: [(UiFontSize, UiTextRole); 6] = [
    (UiFontSize::Large, UiTextRole::Large),
    (UiFontSize::Standard, UiTextRole::Large),
    (UiFontSize::Large, UiTextRole::Heading),
    (UiFontSize::Standard, UiTextRole::Heading),
    (UiFontSize::Compact, UiTextRole::Heading),
    (UiFontSize::Compact, UiTextRole::Body),
];
/// Where the tab is drawn, in portrait logical coordinates. Its right edge
/// lies past the screen edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumeTabBounds {
    pub left: i32,
    pub top: i32,
    pub bottom: i32,
}

/// Compose the sleep frame: `cover` (exactly [`SLEEP_COVER_WIDTH`] x
/// [`SLEEP_COVER_HEIGHT`], bit 1 = ink) plus the resume tab over
/// [`RESUME_TAB_BAND`].
pub fn compose_cover_sleep_frame(
    cover: &CachedThumbnail,
    percent: u8,
    locale: Locale,
) -> (FrameBuffer, ResumeTabBounds) {
    compose_with_band(cover, percent, locale, RESUME_TAB_BAND)
}

fn compose_with_band(
    cover: &CachedThumbnail,
    percent: u8,
    locale: Locale,
    band: (i32, i32),
) -> (FrameBuffer, ResumeTabBounds) {
    let mut frame = FrameBuffer::new_white();
    let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
    let raw = ImageRaw::<BinaryColor>::new(&cover.bits, u32::from(cover.width));
    let drawn = Image::new(&raw, Point::zero())
        .draw(&mut display)
        .and_then(|()| draw_resume_tab(&mut display, percent, locale, band));
    let bounds = match drawn {
        Ok(bounds) => bounds,
        Err(never) => match never {},
    };
    (frame, bounds)
}

fn draw_resume_tab(
    display: &mut OrientedFrameBuffer<'_>,
    percent: u8,
    locale: Locale,
    band: (i32, i32),
) -> Result<ResumeTabBounds, Infallible> {
    let percent_label = format!("{}%", percent.min(100));
    let resume_label = t(locale, "Resume", "Riprendi");

    let (top, band_bottom) = band;
    let inner_height = (band_bottom - top - 2 * TAB_PAD_Y).max(0);

    // Largest strike whose percent fits the band; if none does, the smallest
    // is used and the tab grows down. The label shares it.
    let percent_style = PERCENT_STYLES
        .into_iter()
        .map(|(size, role)| style_for(size, role, BinaryColor::On))
        .find(|candidate| ink_height(*candidate, &percent_label) <= inner_height)
        .unwrap_or_else(|| {
            let (size, role) = PERCENT_STYLES[PERCENT_STYLES.len() - 1];
            style_for(size, role, BinaryColor::On)
        });
    let (ink_top, ink_bottom) = percent_style.text_ink_bounds(&percent_label);
    let bottom = band_bottom.max(top + (ink_bottom - ink_top) + 2 * TAB_PAD_Y);
    let height = bottom - top;
    let arrow_length = arrow_length(percent_style);
    let left = SCREEN_WIDTH
        - (TAB_PAD_X
            + percent_style.text_width(&percent_label)
            + LABEL_GAP
            + percent_style.text_width(resume_label)
            + ARROW_GAP
            + arrow_length
            + TAB_PAD_X);

    let radius = TAB_MAX_RADIUS.min(height as u32 / 2);
    let tab_style = PrimitiveStyleBuilder::new()
        .stroke_color(BinaryColor::On)
        .stroke_width(TAB_STROKE)
        .fill_color(BinaryColor::Off)
        .build();
    // Extend past the right edge so its right corners and stroke are cut off.
    let overhang = radius as i32 + TAB_STROKE as i32;
    RoundedRectangle::with_equal_corners(
        Rectangle::new(
            Point::new(left, top),
            Size::new((SCREEN_WIDTH - left + overhang) as u32, height as u32),
        ),
        Size::new(radius, radius),
    )
    .into_styled(tab_style)
    .draw(display)?;

    // Percent centered on its visible ink; label on the same baseline, arrow
    // centered on the percent's ink.
    let x = left + TAB_PAD_X;
    let baseline = top + (height - (ink_bottom - ink_top)) / 2 - ink_top;
    let percent_end =
        Text::new(&percent_label, Point::new(x, baseline), percent_style).draw(display)?;
    let label_x = percent_end.x.max(x) + LABEL_GAP;
    let text_end =
        Text::new(resume_label, Point::new(label_x, baseline), percent_style).draw(display)?;
    let center_y = baseline + (ink_top + ink_bottom) / 2;
    draw_right_arrow(display, text_end.x.max(label_x) + ARROW_GAP, center_y, arrow_length)?;

    Ok(ResumeTabBounds { left, top, bottom })
}

fn ink_height(style: UiTextStyle, text: &str) -> i32 {
    let (top, bottom) = style.text_ink_bounds(text);
    bottom - top
}

fn arrow_length(style: UiTextStyle) -> i32 {
    (i32::from(style.line_height()) * 4 / 5).max(12)
}

/// "->" arrow centered on `center_y`. The bitmap strikes cover printable
/// ASCII only, so a real U+2192 would draw as `?`.
fn draw_right_arrow(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    center_y: i32,
    length: i32,
) -> Result<(), Infallible> {
    let head = (length * 2 / 5).max(6);
    let fill = PrimitiveStyle::with_fill(BinaryColor::On);
    Rectangle::new(
        Point::new(left, center_y - ARROW_SHAFT / 2),
        Size::new((length - head + 1) as u32, ARROW_SHAFT as u32),
    )
    .into_styled(fill)
    .draw(display)?;
    Triangle::new(
        Point::new(left + length - head, center_y - head * 3 / 4),
        Point::new(left + length, center_y),
        Point::new(left + length - head, center_y + head * 3 / 4),
    )
    .into_styled(fill)
    .draw(display)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::Point;

    use super::{
        compose_cover_sleep_frame, compose_with_band, RESUME_TAB_BAND, SCREEN_WIDTH,
        SLEEP_COVER_HEIGHT, SLEEP_COVER_WIDTH,
    };
    use crate::{
        cover_cache::CachedThumbnail,
        orientation::DisplayOrientation, regional::Locale,
    };

    fn black_cover() -> CachedThumbnail {
        let row_bytes = usize::from(SLEEP_COVER_WIDTH) / 8;
        CachedThumbnail {
            width: SLEEP_COVER_WIDTH,
            height: SLEEP_COVER_HEIGHT,
            bits: vec![0xFF; row_bytes * usize::from(SLEEP_COVER_HEIGHT)],
            placeholder: false,
        }
    }

    fn native(x: i32, y: i32) -> Point {
        DisplayOrientation::Portrait
            .map_logical_to_native(Point::new(x, y))
            .unwrap()
    }

    #[test]
    fn tab_spans_the_band_touches_the_right_edge_and_leaves_the_cover() {
        for (percent, locale) in [(0, Locale::Italian), (42, Locale::English), (100, Locale::Italian)] {
            let band = RESUME_TAB_BAND;
            let (frame, tab) =
                compose_cover_sleep_frame(&black_cover(), percent, locale);
            assert_eq!((tab.top, tab.bottom), band);
            assert!(tab.left > SCREEN_WIDTH / 4 && tab.left < SCREEN_WIDTH - 120);
            // Cover survives outside the tab.
            assert_eq!(frame.is_black(native(4, 400)), Some(true));
            assert_eq!(frame.is_black(native(SCREEN_WIDTH - 1, band.1 + 10)), Some(true));
            // Tab's top stroke reaches the very last column (cut, not rounded).
            assert_eq!(frame.is_black(native(SCREEN_WIDTH - 1, band.0)), Some(true));
            // Just inside the stroke at the right edge the tab is white fill.
            assert_eq!(frame.is_black(native(SCREEN_WIDTH - 1, band.0 + 4)), Some(false));
        }
    }

    #[test]
    fn tiny_band_grows_the_tab_downward_instead_of_clipping_text() {
        let (_, tab) = compose_with_band(
            &black_cover(),
            100,
            Locale::Italian,
            (60, 70),
        );
        assert_eq!(tab.top, 60);
        assert!(tab.bottom > 70);
    }
}
