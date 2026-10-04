//! The progress bar shared by the screens that show how far something got:
//! an audiobook, a download.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::orientation::OrientedFrameBuffer;

/// A 1 px outline filled from the left up to `percent` (capped at 100).
pub fn draw_progress_bar(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    percent: u8,
) -> Result<(), Infallible> {
    Rectangle::new(
        Point::new(left, top),
        Size::new(width.max(0) as u32, height.max(0) as u32),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;
    let filled = (width - 4) * i32::from(percent.min(100)) / 100;
    if filled > 0 {
        Rectangle::new(
            Point::new(left + 2, top + 2),
            Size::new(filled as u32, (height - 4).max(1) as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
    }
    Ok(())
}
