//! List rows, section titles and label/value fields shared by every screen
//! that is not a tile grid.
//!
//! Rows follow the Home dashboard's cards: a rounded border, 1 px at rest
//! and 4 px when selected. Text inside a row is measured and cut to fit, so
//! a long name can never run over the value beside it or leave the screen.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        display::DisplayPreferences,
        typography::{Text, UiTextStyle},
        widgets::{
            layout::{CONTENT_LEFT, CONTENT_RIGHT, CONTENT_WIDTH},
            text::{draw_text_fit, draw_text_right, truncate_to_width, wrap_to_width},
        },
    },
    orientation::OrientedFrameBuffer,
};

/// Height of a single-line row.
pub const ROW_HEIGHT: i32 = 56;
/// Height of a row with a title and a detail line.
pub const TALL_ROW_HEIGHT: i32 = 72;
/// Gap between two rows.
pub const ROW_GAP: i32 = 10;
/// Distance from one single-line row's top to the next.
pub const ROW_STEP: i32 = ROW_HEIGHT + ROW_GAP;
/// Distance from one tall row's top to the next.
pub const TALL_ROW_STEP: i32 = TALL_ROW_HEIGHT + ROW_GAP;
/// Padding between a row's border and its text.
pub const ROW_PAD_X: i32 = 18;
/// Smallest gap kept between a row's label and its value.
const VALUE_GAP: i32 = 14;
/// Corner radius of a row, a little tighter than the Home tiles' 16 px to
/// suit the lower height.
const ROW_CORNER_RADIUS: Size = Size::new(12, 12);
/// Extra space between two label/value fields, on top of the line height.
const FIELD_GAP: i32 = 14;
/// Indent of a field value that had to move to its own line.
const FIELD_VALUE_INDENT: i32 = 16;

/// The rounded border of a row spanning the content width.
pub fn draw_row_frame(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    height: i32,
    selected: bool,
) -> Result<(), Infallible> {
    draw_row_frame_at(display, CONTENT_LEFT, top, CONTENT_WIDTH, height, selected)
}

/// The rounded border of a row at any position and width.
pub fn draw_row_frame_at(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    selected: bool,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(left, top),
            Size::new(width.max(0) as u32, height.max(0) as u32),
        ),
        CornerRadii::new(ROW_CORNER_RADIUS),
    )
    .into_styled(border)
    .draw(display)
}

/// Baseline that centers one line of `style` in a box `height` tall
/// starting at `top`, by cap height so every row lines up alike whatever
/// letters its label happens to hold.
#[must_use]
pub fn centered_baseline(style: UiTextStyle, top: i32, height: i32) -> i32 {
    let (cap_top, _) = style.text_ink_bounds("H");
    let cap_height = -cap_top;
    top + (height + cap_height) / 2
}

/// A single-line row: `label` on the left, `value` (may be empty) ending at
/// the right. The value keeps up to half the row; the label takes the rest
/// and is cut with an ellipsis.
pub fn draw_list_row(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    top: i32,
    label: &str,
    value: &str,
    selected: bool,
) -> Result<(), Infallible> {
    draw_row_frame(display, top, ROW_HEIGHT, selected)?;
    let style = preferences.body_style();
    let baseline = centered_baseline(style, top, ROW_HEIGHT);
    let inner_left = CONTENT_LEFT + ROW_PAD_X;
    let inner_right = CONTENT_RIGHT - ROW_PAD_X;
    let inner_width = inner_right - inner_left;
    let mut label_width = inner_width;
    if !value.is_empty() {
        let fitted = truncate_to_width(style, value, inner_width / 2);
        let value_width = style.text_width(&fitted);
        Text::new(
            &fitted,
            Point::new(inner_right - value_width, baseline),
            style,
        )
        .draw(display)?;
        label_width = inner_width - value_width - VALUE_GAP;
    }
    draw_text_fit(
        display,
        label,
        Point::new(inner_left, baseline),
        style,
        label_width,
    )
}

/// A tall row: `title` over a smaller `detail` line, with an optional short
/// `trailing` note at the right of the title line.
#[allow(clippy::too_many_arguments)]
pub fn draw_tall_row(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    top: i32,
    title: &str,
    detail: &str,
    trailing: &str,
    selected: bool,
) -> Result<(), Infallible> {
    draw_row_frame(display, top, TALL_ROW_HEIGHT, selected)?;
    let title_style = preferences.heading_style();
    let detail_style = preferences.detail_style();
    let inner_left = CONTENT_LEFT + ROW_PAD_X;
    let inner_right = CONTENT_RIGHT - ROW_PAD_X;
    let inner_width = inner_right - inner_left;
    let title_baseline = top + 30;
    let mut title_width = inner_width;
    if !trailing.is_empty() {
        let fitted = truncate_to_width(detail_style, trailing, inner_width / 2);
        let trailing_width = detail_style.text_width(&fitted);
        Text::new(
            &fitted,
            Point::new(inner_right - trailing_width, title_baseline),
            detail_style,
        )
        .draw(display)?;
        title_width = inner_width - trailing_width - VALUE_GAP;
    }
    draw_text_fit(
        display,
        title,
        Point::new(inner_left, title_baseline),
        title_style,
        title_width,
    )?;
    if !detail.is_empty() {
        draw_text_fit(
            display,
            detail,
            Point::new(inner_left, top + 56),
            detail_style,
            inner_width,
        )?;
    }
    Ok(())
}

/// A section title in the heading style. Returns the baseline of a first
/// field below it.
pub fn draw_section_title(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    baseline: i32,
    text: &str,
) -> Result<i32, Infallible> {
    let style = preferences.heading_style();
    draw_text_fit(
        display,
        text,
        Point::new(CONTENT_LEFT, baseline),
        style,
        CONTENT_WIDTH,
    )?;
    Ok(baseline + i32::from(preferences.body_style().line_height()) + FIELD_GAP + 4)
}

/// One read-only field: `label` on the left, `value` ending at the right
/// edge. When the two do not fit on one line the value moves below the
/// label, wrapped to at most two lines. Returns the next field's baseline.
pub fn draw_field(
    display: &mut OrientedFrameBuffer<'_>,
    preferences: DisplayPreferences,
    baseline: i32,
    label: &str,
    value: &str,
) -> Result<i32, Infallible> {
    let style = preferences.body_style();
    let line = i32::from(style.line_height());
    let label_width = style.text_width(label);
    let value_width = style.text_width(value);
    if label_width + VALUE_GAP + value_width <= CONTENT_WIDTH {
        Text::new(label, Point::new(CONTENT_LEFT, baseline), style).draw(display)?;
        draw_text_right(
            display,
            value,
            CONTENT_RIGHT,
            baseline,
            style,
            CONTENT_WIDTH,
        )?;
        return Ok(baseline + line + FIELD_GAP);
    }
    draw_text_fit(
        display,
        label,
        Point::new(CONTENT_LEFT, baseline),
        style,
        CONTENT_WIDTH,
    )?;
    let mut next = baseline + line + 4;
    for wrapped in wrap_to_width(style, value, CONTENT_WIDTH - FIELD_VALUE_INDENT, 2) {
        Text::new(
            &wrapped,
            Point::new(CONTENT_LEFT + FIELD_VALUE_INDENT, next),
            style,
        )
        .draw(display)?;
        next += line + 4;
    }
    Ok(next + FIELD_GAP - 4)
}

/// The page of a list that holds `selected`, for lists shown `per_page`
/// rows at a time: `(first index, end index, page number from 1, pages)`.
#[must_use]
pub fn page_window(selected: usize, count: usize, per_page: usize) -> (usize, usize, usize, usize) {
    if count == 0 || per_page == 0 {
        return (0, 0, 1, 1);
    }
    let selected = selected.min(count - 1);
    let page = selected / per_page;
    let first = page * per_page;
    let end = (first + per_page).min(count);
    (first, end, page + 1, count.div_ceil(per_page))
}

#[cfg(test)]
mod tests {
    use super::page_window;

    #[test]
    fn page_window_follows_the_selection() {
        assert_eq!(page_window(0, 0, 8), (0, 0, 1, 1));
        assert_eq!(page_window(3, 5, 8), (0, 5, 1, 1));
        assert_eq!(page_window(8, 12, 8), (8, 12, 2, 2));
        assert_eq!(page_window(99, 12, 8), (8, 12, 2, 2));
        assert_eq!(page_window(7, 16, 8), (0, 8, 1, 2));
    }
}
