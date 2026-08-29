//! Logical display orientation for the native 800 × 480 e-paper framebuffer.
//!
//! The panel transport always remains 800 × 480. Product-facing screens draw
//! into an orientation-aware target so application code does not need to know
//! how logical coordinates map into the packed native panel buffer.

use core::convert::Infallible;

use embedded_graphics::{
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
    prelude::{DrawTarget, Pixel, Point},
};

use crate::framebuffer::{FrameBuffer, HEIGHT, ROW_BYTES, WIDTH};

/// Supported logical UI orientations over the native 800 × 480 panel buffer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisplayOrientation {
    /// Native 800 × 480 landscape coordinates.
    Landscape,
    /// Upright 480 × 800 product layout with the USB connector at the bottom.
    #[default]
    Portrait,
    /// Native landscape coordinates rotated by 180 degrees.
    LandscapeInverted,
    /// Portrait coordinates rotated by 180 degrees.
    PortraitInverted,
}

impl DisplayOrientation {
    /// Return the logical drawing dimensions for this orientation.
    #[must_use]
    pub const fn logical_size(self) -> Size {
        match self {
            Self::Landscape | Self::LandscapeInverted => Size::new(WIDTH, HEIGHT),
            Self::Portrait | Self::PortraitInverted => Size::new(HEIGHT, WIDTH),
        }
    }

    /// Return a short human-readable orientation name for diagnostics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Landscape => "Landscape",
            Self::Portrait => "Portrait",
            Self::LandscapeInverted => "Landscape inverted",
            Self::PortraitInverted => "Portrait inverted",
        }
    }

    /// Transform a logical screen point into native panel coordinates.
    ///
    /// Points outside the logical drawing surface are discarded before they
    /// can reach the packed native framebuffer.
    #[must_use]
    pub fn map_logical_to_native(self, point: Point) -> Option<Point> {
        let size = self.logical_size();
        if point.x < 0
            || point.y < 0
            || point.x >= size.width as i32
            || point.y >= size.height as i32
        {
            return None;
        }

        let x = point.x;
        let y = point.y;
        Some(match self {
            Self::Landscape => Point::new(x, y),
            Self::Portrait => Point::new(y, HEIGHT as i32 - 1 - x),
            Self::LandscapeInverted => Point::new(WIDTH as i32 - 1 - x, HEIGHT as i32 - 1 - y),
            Self::PortraitInverted => Point::new(WIDTH as i32 - 1 - y, x),
        })
    }
}

/// Orientation-aware embedded-graphics target backed by a native framebuffer.
pub struct OrientedFrameBuffer<'a> {
    frame: &'a mut FrameBuffer,
    orientation: DisplayOrientation,
}

impl<'a> OrientedFrameBuffer<'a> {
    /// Wrap a native framebuffer with a logical display orientation.
    #[must_use]
    pub fn new(frame: &'a mut FrameBuffer, orientation: DisplayOrientation) -> Self {
        Self { frame, orientation }
    }

    /// Return the active logical orientation.
    #[must_use]
    pub const fn orientation(&self) -> DisplayOrientation {
        self.orientation
    }

    /// Fast-path blit of a packed 1bpp bitmap (row-major, MSB-first, bit `1`
    /// = ink) straight into the backing framebuffer, bypassing
    /// embedded-graphics' generic per-pixel `Image`/`Drawable` iteration
    /// chain. Only implements the `Portrait` coordinate transform — the only
    /// orientation the Library grid (its one caller) ever runs under, see
    /// [`crate::app::state::AppState`]'s route/orientation sync — so callers
    /// must check [`Self::orientation`] first and fall back to the generic
    /// `Image` draw for any other orientation; misuse is caught by a
    /// `debug_assert` rather than risking a release-build panic.
    ///
    /// Pixels already white need no write: the caller always draws onto a
    /// freshly `clear_white`d frame, so only ink bits are ever set.
    pub fn blit_packed_bitmap_portrait(&mut self, top_left: Point, width: u16, height: u16, bits: &[u8]) {
        debug_assert_eq!(self.orientation, DisplayOrientation::Portrait);
        let width = i32::from(width);
        let height = i32::from(height);
        if width <= 0 || height <= 0 {
            return;
        }
        let src_row_bytes = (width as usize).div_ceil(8);
        debug_assert!(bits.len() >= src_row_bytes * height as usize);

        // Portrait maps logical (x, y) to native (y, HEIGHT - 1 - x). A
        // source row (fixed local y) therefore always lands at a single
        // native column, and a source column (local x) always lands at the
        // same native row regardless of which source row it came from — so
        // both the destination byte-column and the valid column range can
        // be hoisted out of the row loop entirely.
        let ny_base = HEIGHT as i32 - 1 - top_left.x;
        let col_start = (ny_base - (HEIGHT as i32 - 1)).max(0);
        let col_end = ny_base.min(width - 1);
        if col_end < col_start {
            return;
        }

        let frame_bytes = self.frame.bytes_mut();
        for row in 0..height {
            let nx = top_left.y + row;
            if nx < 0 || nx >= WIDTH as i32 {
                continue;
            }
            let byte_col = nx as usize / 8;
            let dest_mask = 0x80u8 >> (nx as usize % 8);
            let src_row_offset = row as usize * src_row_bytes;
            let mut byte_index = (ny_base - col_start) as usize * ROW_BYTES + byte_col;
            for col in col_start..=col_end {
                let src_byte = bits[src_row_offset + col as usize / 8];
                let src_mask = 0x80u8 >> (col as usize % 8);
                if src_byte & src_mask != 0 {
                    frame_bytes[byte_index] &= !dest_mask;
                }
                byte_index -= ROW_BYTES;
            }
        }
    }
}

impl OriginDimensions for OrientedFrameBuffer<'_> {
    fn size(&self) -> Size {
        self.orientation.logical_size()
    }
}

impl DrawTarget for OrientedFrameBuffer<'_> {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(logical_point, color) in pixels {
            let Some(native_point) = self.orientation.map_logical_to_native(logical_point) else {
                continue;
            };
            self.frame
                .set_native_black(native_point, color == BinaryColor::On);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{
        pixelcolor::BinaryColor,
        prelude::{DrawTarget, Pixel, Point},
    };

    use super::{DisplayOrientation, OrientedFrameBuffer};
    use crate::framebuffer::FrameBuffer;

    #[test]
    fn blit_packed_bitmap_portrait_matches_generic_image_draw() {
        use embedded_graphics::{
            image::{Image, ImageRaw},
            prelude::Drawable,
        };

        // 8 x 3 bitmap, row-major MSB-first, exercising a non-trivial
        // pattern across full-byte rows.
        let width: u16 = 8;
        let height: u16 = 3;
        let bits: [u8; 3] = [0b1010_0101, 0b0000_1111, 0b1111_0000];
        let top_left = Point::new(40, 60);

        let mut frame_generic = FrameBuffer::new_white();
        {
            let mut display =
                OrientedFrameBuffer::new(&mut frame_generic, DisplayOrientation::Portrait);
            let raw = ImageRaw::<BinaryColor>::new(&bits, u32::from(width));
            Image::new(&raw, top_left).draw(&mut display).unwrap();
        }

        let mut frame_fast = FrameBuffer::new_white();
        {
            let mut display =
                OrientedFrameBuffer::new(&mut frame_fast, DisplayOrientation::Portrait);
            display.blit_packed_bitmap_portrait(top_left, width, height, &bits);
        }

        assert_eq!(frame_generic.as_bytes(), frame_fast.as_bytes());
    }

    #[test]
    fn portrait_is_the_product_default() {
        assert_eq!(DisplayOrientation::default(), DisplayOrientation::Portrait);
        assert_eq!(DisplayOrientation::Portrait.logical_size().width, 480);
        assert_eq!(DisplayOrientation::Portrait.logical_size().height, 800);
    }

    #[test]
    fn portrait_maps_logical_corners_to_native_buffer() {
        let orientation = DisplayOrientation::Portrait;
        assert_eq!(
            orientation.map_logical_to_native(Point::new(0, 0)),
            Some(Point::new(0, 479))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(479, 0)),
            Some(Point::new(0, 0))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(0, 799)),
            Some(Point::new(799, 479))
        );
        assert_eq!(
            orientation.map_logical_to_native(Point::new(479, 799)),
            Some(Point::new(799, 0))
        );
    }

    #[test]
    fn all_orientation_mappings_stay_inside_native_frame() {
        for orientation in [
            DisplayOrientation::Landscape,
            DisplayOrientation::Portrait,
            DisplayOrientation::LandscapeInverted,
            DisplayOrientation::PortraitInverted,
        ] {
            let size = orientation.logical_size();
            for point in [
                Point::new(0, 0),
                Point::new(size.width as i32 - 1, 0),
                Point::new(0, size.height as i32 - 1),
                Point::new(size.width as i32 - 1, size.height as i32 - 1),
            ] {
                let mapped = orientation.map_logical_to_native(point).unwrap();
                assert!((0..800).contains(&mapped.x));
                assert!((0..480).contains(&mapped.y));
            }
        }
    }

    #[test]
    fn out_of_bounds_logical_points_are_rejected() {
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(-1, 0)),
            None
        );
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(480, 0)),
            None
        );
        assert_eq!(
            DisplayOrientation::Portrait.map_logical_to_native(Point::new(0, 800)),
            None
        );
    }

    #[test]
    fn oriented_target_writes_to_native_framebuffer() {
        let mut frame = FrameBuffer::new_white();
        let mut oriented = OrientedFrameBuffer::new(&mut frame, DisplayOrientation::Portrait);
        oriented
            .draw_iter([Pixel(Point::new(0, 0), BinaryColor::On)])
            .unwrap();
        assert_eq!(frame.is_black(Point::new(0, 479)), Some(true));
    }
}
