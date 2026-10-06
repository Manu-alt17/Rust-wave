//! Sleep screen of a card without wallpapers: the steps to add one.
//!
//! Standby used to fall back to an empty frame when `RUSTMIX/SLEEP` held no
//! usable image, which said nothing about what belongs there. This frame
//! takes its place and tells how a wallpaper gets on the card: from the
//! phone, through the Wi-Fi page.
//!
//! Drawn through the same portrait mapping as every product screen, in the
//! interface text size the user chose, and stacked by line height so no size
//! pushes one line into the next.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{
        Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle, Triangle,
    },
};

use crate::{
    app::{
        display::{SleepScreenMode, UiFontSize},
        i18n::t,
        typography::{style_for, Text, UiTextRole, UiTextStyle},
        widgets::text::wrap_to_width,
    },
    framebuffer::FrameBuffer,
    orientation::{DisplayOrientation, OrientedFrameBuffer},
    regional::Locale,
};

const SCREEN_WIDTH: i32 = DisplayOrientation::Portrait.logical_size().width as i32;
const SCREEN_HEIGHT: i32 = DisplayOrientation::Portrait.logical_size().height as i32;
/// Side margin of the text column: wider than a menu's, since nothing here
/// is a row to select.
const MARGIN: i32 = 36;
const COLUMN_WIDTH: i32 = SCREEN_WIDTH - 2 * MARGIN;

/// Where the page starts when measured, and the least it leaves above it.
const ICON_TOP: i32 = 52;
const ICON_WIDTH: i32 = 132;
const ICON_HEIGHT: i32 = 100;
/// Same stroke as the Home tile icons.
const ICON_STROKE: u32 = 5;
const ICON_RADIUS: u32 = 14;

const LINE_GAP: i32 = 6;
const STEP_GAP: i32 = 14;
const BADGE_TEXT_GAP: i32 = 14;
/// Most lines one step or paragraph may take: far more than any of the
/// texts needs at the largest size, so nothing is ever cut with an ellipsis.
const MAX_LINES: usize = 6;

/// The frame standby shows when there is no wallpaper to show: what is
/// missing and the four steps to add one. `mode` is the sleep screen the
/// user chose, which decides the closing note: the book cover is offered as
/// the other choice, unless it is the one already chosen.
#[must_use]
pub fn compose_no_wallpaper_frame(
    locale: Locale,
    font_size: UiFontSize,
    mode: SleepScreenMode,
) -> FrameBuffer {
    let mut frame = FrameBuffer::new_white();
    // Drawn once to learn how tall the page is at this size and language,
    // then again where it sits in the middle of the screen.
    let end = draw_on(&mut frame, locale, font_size, mode, ICON_TOP);
    let top = centered_top(end);
    if top != ICON_TOP {
        frame.clear_white();
        draw_on(&mut frame, locale, font_size, mode, top);
    }
    frame
}

fn draw_on(
    frame: &mut FrameBuffer,
    locale: Locale,
    font_size: UiFontSize,
    mode: SleepScreenMode,
    top: i32,
) -> i32 {
    let mut display = OrientedFrameBuffer::new(frame, DisplayOrientation::Portrait);
    match draw_tutorial(&mut display, locale, font_size, mode, top) {
        Ok(end) => end,
        Err(never) => match never {},
    }
}

/// Top of a page that ended at `end` when drawn from [`ICON_TOP`]: the page
/// is set a little above the middle, where the eye takes it as centered.
fn centered_top(end: i32) -> i32 {
    let height = end - ICON_TOP;
    ((SCREEN_HEIGHT - height) * 2 / 5).max(ICON_TOP)
}

/// Draws the page from `top` and returns the y just below its last line.
fn draw_tutorial(
    display: &mut OrientedFrameBuffer<'_>,
    locale: Locale,
    font_size: UiFontSize,
    mode: SleepScreenMode,
    top: i32,
) -> Result<i32, Infallible> {
    let title_style = style_for(font_size, UiTextRole::Large, BinaryColor::On);
    let body = style_for(font_size, UiTextRole::Body, BinaryColor::On);
    let body_line = i32::from(body.line_height());

    draw_picture_icon(display, (SCREEN_WIDTH - ICON_WIDTH) / 2, top)?;

    let title = t(locale, "No wallpaper yet", "Nessuno sfondo");
    let mut baseline = top + ICON_HEIGHT + 30 + i32::from(title_style.line_height());
    Text::new(
        title,
        Point::new((SCREEN_WIDTH - title_style.text_width(title)) / 2, baseline),
        title_style,
    )
    .draw(display)?;

    baseline += 16 + body_line;
    baseline = draw_lines(
        display,
        t(
            locale,
            "A picture of yours can stay here in standby. You add it from a phone:",
            "Qui, in standby, pu\u{00F2} restare una tua immagine. Si aggiunge dal telefono:",
        ),
        MARGIN,
        baseline,
        body,
        COLUMN_WIDTH,
    )?;

    let steps = [
        t(
            locale,
            "Press Power to turn the device back on.",
            "Premi Power per riaccendere il dispositivo.",
        ),
        t(
            locale,
            "From Home open Upload, then Wi-Fi.",
            "Dalla Home apri Carica, poi Wi-Fi.",
        ),
        t(
            locale,
            "Point the phone at the QR code.",
            "Inquadra il codice QR con il telefono.",
        ),
        t(
            locale,
            "On the page open Wallpapers and upload a picture.",
            "Nella pagina apri Sfondi e carica un'immagine.",
        ),
    ];
    let badge = body_line + 8;
    let text_left = MARGIN + badge + BADGE_TEXT_GAP;
    baseline += STEP_GAP;
    for (index, step) in steps.iter().enumerate() {
        draw_number_badge(display, MARGIN, baseline, badge, index + 1, font_size)?;
        baseline = draw_lines(
            display,
            step,
            text_left,
            baseline,
            body,
            SCREEN_WIDTH - MARGIN - text_left,
        )? + STEP_GAP;
    }

    // A rule, then the other way to fill this screen.
    let rule_y = baseline - body_line + STEP_GAP / 2;
    Rectangle::new(
        Point::new(MARGIN, rule_y),
        Size::new(COLUMN_WIDTH as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;
    baseline = rule_y + STEP_GAP + body_line;
    let note = if mode == SleepScreenMode::BookCover {
        t(
            locale,
            "The cover of your book shows here when standby starts while you are reading.",
            "La copertina del libro compare qui quando vai in standby mentre leggi.",
        )
    } else {
        t(
            locale,
            "Or show the cover of the book you are reading: Settings, Display, Sleep screen.",
            "Oppure mostra la copertina del libro che stai leggendo: Opzioni, Schermo, Schermata di standby.",
        )
    };
    let end = draw_lines(display, note, MARGIN, baseline, body, COLUMN_WIDTH)?;
    Ok(end - body_line)
}

/// Word-wrapped text from `first_baseline`; returns the baseline the next
/// line would take.
fn draw_lines(
    display: &mut OrientedFrameBuffer<'_>,
    text: &str,
    left: i32,
    first_baseline: i32,
    style: UiTextStyle,
    max_width: i32,
) -> Result<i32, Infallible> {
    let step = i32::from(style.line_height()) + LINE_GAP;
    let mut baseline = first_baseline;
    for line in wrap_to_width(style, text, max_width, MAX_LINES) {
        Text::new(&line, Point::new(left, baseline), style).draw(display)?;
        baseline += step;
    }
    Ok(baseline)
}

/// Step number in a filled disc, level with the first line of its text.
fn draw_number_badge(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    text_baseline: i32,
    diameter: i32,
    number: usize,
    font_size: UiFontSize,
) -> Result<(), Infallible> {
    let ink = style_for(font_size, UiTextRole::Body, BinaryColor::On);
    let paper = style_for(font_size, UiTextRole::Heading, BinaryColor::Off);
    // Centered on the ink of the line it numbers.
    let (line_top, line_bottom) = ink.text_ink_bounds("H");
    let center_y = text_baseline + (line_top + line_bottom) / 2;
    Circle::new(Point::new(left, center_y - diameter / 2), diameter as u32)
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    let label = number.to_string();
    let (top, bottom) = paper.text_ink_bounds(&label);
    Text::new(
        &label,
        Point::new(
            left + (diameter - paper.text_width(&label)) / 2,
            center_y - (top + bottom) / 2,
        ),
        paper,
    )
    .draw(display)?;
    Ok(())
}

/// A framed picture (sun and two hills) with a "+" disc on its corner: a
/// picture to add.
fn draw_picture_icon(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    top: i32,
) -> Result<(), Infallible> {
    let ink = PrimitiveStyle::with_fill(BinaryColor::On);
    let paper = PrimitiveStyle::with_fill(BinaryColor::Off);
    let floor = top + ICON_HEIGHT - 16;

    // Hills first, so the frame's stroke closes them at the bottom.
    Triangle::new(
        Point::new(left + 14, floor),
        Point::new(left + 50, top + 42),
        Point::new(left + 86, floor),
    )
    .into_styled(ink)
    .draw(display)?;
    Triangle::new(
        Point::new(left + 62, floor),
        Point::new(left + 88, top + 58),
        Point::new(left + 114, floor),
    )
    .into_styled(ink)
    .draw(display)?;
    Circle::new(Point::new(left + 84, top + 20), 20)
        .into_styled(ink)
        .draw(display)?;
    RoundedRectangle::with_equal_corners(
        Rectangle::new(
            Point::new(left, top),
            Size::new(ICON_WIDTH as u32, ICON_HEIGHT as u32),
        ),
        Size::new(ICON_RADIUS, ICON_RADIUS),
    )
    .into_styled(
        PrimitiveStyleBuilder::new()
            .stroke_color(BinaryColor::On)
            .stroke_width(ICON_STROKE)
            .build(),
    )
    .draw(display)?;

    // "+" disc over the bottom right corner, ringed in white.
    let disc = 44;
    let disc_left = left + ICON_WIDTH - disc / 2 - 6;
    let disc_top = top + ICON_HEIGHT - disc / 2 - 6;
    Circle::new(Point::new(disc_left - 5, disc_top - 5), (disc + 10) as u32)
        .into_styled(paper)
        .draw(display)?;
    Circle::new(Point::new(disc_left, disc_top), disc as u32)
        .into_styled(ink)
        .draw(display)?;
    let center = Point::new(disc_left + disc / 2, disc_top + disc / 2);
    let arm = 11;
    let thickness = 5;
    Rectangle::new(
        Point::new(center.x - arm, center.y - thickness / 2),
        Size::new((2 * arm + 1) as u32, thickness as u32),
    )
    .into_styled(paper)
    .draw(display)?;
    Rectangle::new(
        Point::new(center.x - thickness / 2, center.y - arm),
        Size::new(thickness as u32, (2 * arm + 1) as u32),
    )
    .into_styled(paper)
    .draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::Point;

    use super::{
        centered_top, compose_no_wallpaper_frame, draw_tutorial, ICON_TOP, MARGIN, SCREEN_HEIGHT,
        SCREEN_WIDTH,
    };
    use crate::{
        app::{
            display::{SleepScreenMode, UiFontSize},
            typography::audit,
            widgets::text::ELLIPSIS,
        },
        framebuffer::FrameBuffer,
        orientation::{DisplayOrientation, OrientedFrameBuffer},
        regional::Locale,
    };

    const SIZES: [UiFontSize; 3] = [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large];
    const MODES: [SleepScreenMode; 4] = [
        SleepScreenMode::Sequential,
        SleepScreenMode::Random,
        SleepScreenMode::Fixed,
        SleepScreenMode::BookCover,
    ];

    /// Every string drawn where the frame has it, the top of the page and
    /// the y just below its last line.
    fn drawn(
        locale: Locale,
        size: UiFontSize,
        mode: SleepScreenMode,
    ) -> (Vec<audit::Rec>, i32, i32) {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
        let measured = draw_tutorial(&mut display, locale, size, mode, ICON_TOP).unwrap();
        let top = centered_top(measured);
        audit::start();
        let end = draw_tutorial(&mut display, locale, size, mode, top).unwrap();
        (audit::take(), top, end)
    }

    #[test]
    fn every_line_stays_on_the_screen_whole_and_clear_of_the_others() {
        for locale in [Locale::Italian, Locale::English] {
            for size in SIZES {
                for mode in MODES {
                    let (records, top, end) = drawn(locale, size, mode);
                    let tag = format!("{} {} {}", locale.name(), size.marker(), mode.marker());
                    assert!(top >= ICON_TOP, "{tag}: starts at {top}");
                    assert!(end <= SCREEN_HEIGHT - ICON_TOP, "{tag}: ends at {end}");
                    // More room below than above, never the other way round.
                    assert!(
                        SCREEN_HEIGHT - end >= top,
                        "{tag}: {top} above, ends at {end}"
                    );
                    for rec in &records {
                        assert!(
                            rec.x >= MARGIN,
                            "{tag}: {:?} starts in the margin",
                            rec.text
                        );
                        assert!(
                            rec.x + rec.width <= SCREEN_WIDTH - MARGIN,
                            "{tag}: {:?} runs past the margin",
                            rec.text
                        );
                        assert!(rec.ink_bottom < SCREEN_HEIGHT, "{tag}: {:?}", rec.text);
                        assert!(!rec.text.contains(ELLIPSIS), "{tag}: {:?} is cut", rec.text);
                        assert!(!rec.text.contains('?'), "{tag}: {:?}", rec.text);
                    }
                    for (index, a) in records.iter().enumerate() {
                        for b in records.iter().skip(index + 1) {
                            let horizontal = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
                            let vertical =
                                a.ink_bottom.min(b.ink_bottom) - a.ink_top.max(b.ink_top);
                            assert!(
                                horizontal <= 2 || vertical <= 2,
                                "{tag}: {:?} over {:?}",
                                a.text,
                                b.text
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn it_says_how_to_add_a_wallpaper_in_the_language_of_the_device() {
        let text = |locale| {
            drawn(locale, UiFontSize::Standard, SleepScreenMode::Sequential)
                .0
                .iter()
                .map(|rec| rec.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let italian = text(Locale::Italian);
        for word in ["Nessuno sfondo", "Power", "Carica", "Wi-Fi", "QR", "Sfondi"] {
            assert!(italian.contains(word), "missing {word}: {italian}");
        }
        let english = text(Locale::English);
        for word in [
            "No wallpaper",
            "Power",
            "Upload",
            "Wi-Fi",
            "QR",
            "Wallpapers",
        ] {
            assert!(english.contains(word), "missing {word}: {english}");
        }
    }

    #[test]
    fn the_book_cover_is_offered_only_to_who_has_not_chosen_it() {
        let note = |mode| {
            drawn(Locale::Italian, UiFontSize::Standard, mode)
                .0
                .iter()
                .map(|rec| rec.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(note(SleepScreenMode::Sequential).contains("Opzioni, Schermo"));
        assert!(!note(SleepScreenMode::BookCover).contains("Opzioni, Schermo"));
        assert!(note(SleepScreenMode::BookCover).contains("mentre leggi"));
    }

    #[test]
    fn the_frame_is_drawn_upright_with_ink_on_white() {
        let frame = compose_no_wallpaper_frame(
            Locale::Italian,
            UiFontSize::Standard,
            SleepScreenMode::Sequential,
        );
        let native = |x, y| {
            DisplayOrientation::Portrait
                .map_logical_to_native(Point::new(x, y))
                .unwrap()
        };
        // Corners stay white; the picture's frame is inked at the top of the
        // page, in the middle of the screen's width, and nothing above it.
        let (_, top, _) = drawn(
            Locale::Italian,
            UiFontSize::Standard,
            SleepScreenMode::Sequential,
        );
        assert_eq!(frame.is_black(native(2, 2)), Some(false));
        assert_eq!(
            frame.is_black(native(SCREEN_WIDTH - 3, SCREEN_HEIGHT - 3)),
            Some(false)
        );
        assert_eq!(
            frame.is_black(native(SCREEN_WIDTH / 2 - 20, top + 2)),
            Some(true)
        );
        assert_eq!(
            frame.is_black(native(SCREEN_WIDTH / 2 - 20, top - 4)),
            Some(false)
        );
        let ink = frame
            .as_bytes()
            .iter()
            .filter(|byte| **byte != 0xFF)
            .count();
        assert!(ink > 500, "only {ink} bytes carry ink");
    }

    /// `SLEEP_TUTORIAL_OUT=<dir>` saves the frame as PNG in every language
    /// and size, for a look at it.
    #[test]
    fn saves_previews_on_request() {
        let Some(dir) = std::env::var_os("SLEEP_TUTORIAL_OUT").map(std::path::PathBuf::from) else {
            return;
        };
        std::fs::create_dir_all(&dir).unwrap();
        for locale in [Locale::Italian, Locale::English] {
            for size in SIZES {
                for mode in [SleepScreenMode::Sequential, SleepScreenMode::BookCover] {
                    let frame = compose_no_wallpaper_frame(locale, size, mode);
                    let mut canvas =
                        image::GrayImage::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
                    for y in 0..SCREEN_HEIGHT {
                        for x in 0..SCREEN_WIDTH {
                            let black = DisplayOrientation::Portrait
                                .map_logical_to_native(Point::new(x, y))
                                .and_then(|native| frame.is_black(native))
                                .unwrap_or(false);
                            canvas.put_pixel(
                                x as u32,
                                y as u32,
                                image::Luma([if black { 0 } else { 255 }]),
                            );
                        }
                    }
                    canvas
                        .save(dir.join(format!(
                            "{}-{}-{}.png",
                            locale.name(),
                            size.marker(),
                            mode.marker()
                        )))
                        .unwrap();
                }
            }
        }
    }
}
