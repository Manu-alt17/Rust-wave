//! "RIATTIVAZIONE" wake-in-progress overlay drawn over the retained sleep
//! image immediately after a real hardware deep-sleep reboot (SELECT/GPIO5),
//! before the rest of boot (SD/network/sensor init) has run. E-paper holds
//! its image with no power applied, so the sleep-image frame is still
//! physically on the glass at this point; this box is the only thing that
//! needs drawing until the final Home refresh replaces the whole screen once
//! boot is ready.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyleBuilder, Rectangle},
};

use crate::{
    app::{display::DisplayPreferences, typography::Text},
    framebuffer::FrameBuffer,
    orientation::{DisplayOrientation, OrientedFrameBuffer},
};

/// Reactivation label shown while boot finishes after a real deep-sleep wake.
pub const WAKE_OVERLAY_LABEL: &str = "RIATTIVAZIONE";

const OVERLAY_WIDTH: i32 = 360;
const OVERLAY_HEIGHT: i32 = 100;
const OVERLAY_TEXT_BASELINE_OFFSET: i32 = 64;
const LOGICAL_WIDTH: i32 = DisplayOrientation::Portrait.logical_size().width as i32;
const LOGICAL_HEIGHT: i32 = DisplayOrientation::Portrait.logical_size().height as i32;
const OVERLAY_LEFT: i32 = (LOGICAL_WIDTH - OVERLAY_WIDTH) / 2;
const OVERLAY_TOP: i32 = (LOGICAL_HEIGHT - OVERLAY_HEIGHT) / 2;

/// Draw a bordered, opaque box with [`WAKE_OVERLAY_LABEL`] over whatever the
/// frame already holds. Callers pass a reloaded copy of the sleep-image
/// frame so the box reads as an overlay on top of it, matching what is still
/// physically shown on the e-paper glass at boot.
///
/// Sleep images are stored pre-rotated straight into the native 800 × 480
/// buffer (see `sleep_images`), which is how they appear upright on the
/// physically portrait-mounted panel. This overlay must draw through the
/// same [`DisplayOrientation::Portrait`] mapping every other product screen
/// uses, not raw native coordinates, or the box and label come out rotated
/// 90 degrees relative to the sleep image underneath.
pub fn draw_wake_overlay(frame: &mut FrameBuffer) -> Result<(), Infallible> {
    let mut display = OrientedFrameBuffer::new(frame, DisplayOrientation::Portrait);

    let box_style = PrimitiveStyleBuilder::new()
        .stroke_color(BinaryColor::On)
        .stroke_width(3)
        .fill_color(BinaryColor::Off)
        .build();
    Rectangle::new(
        Point::new(OVERLAY_LEFT, OVERLAY_TOP),
        Size::new(OVERLAY_WIDTH as u32, OVERLAY_HEIGHT as u32),
    )
    .into_styled(box_style)
    .draw(&mut display)?;

    let style = DisplayPreferences::default().heading_style();
    let text_left = OVERLAY_LEFT + (OVERLAY_WIDTH - style.text_width(WAKE_OVERLAY_LABEL)) / 2;
    Text::new(
        WAKE_OVERLAY_LABEL,
        Point::new(text_left, OVERLAY_TOP + OVERLAY_TEXT_BASELINE_OFFSET),
        style,
    )
    .draw(&mut display)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use embedded_graphics::prelude::Point;

    use super::{draw_wake_overlay, OVERLAY_LEFT, OVERLAY_TOP, OVERLAY_WIDTH};
    use crate::{framebuffer::FrameBuffer, orientation::DisplayOrientation};

    /// Map a logical portrait point to the native panel point the overlay
    /// actually draws into, mirroring the mapping `draw_wake_overlay` uses.
    fn native(x: i32, y: i32) -> Point {
        DisplayOrientation::Portrait
            .map_logical_to_native(Point::new(x, y))
            .unwrap()
    }

    #[test]
    fn draws_a_visible_box_and_label_on_a_blank_frame() {
        let mut frame = FrameBuffer::new_white();
        draw_wake_overlay(&mut frame).unwrap();
        assert!(frame.as_bytes().iter().any(|byte| *byte != 0xFF));
    }

    #[test]
    fn box_fill_occludes_whatever_was_underneath() {
        // A background pixel deep inside the box footprint (but away from the
        // centered label glyphs) must read back as white once the opaque box
        // fill is drawn, regardless of what the background held before.
        let mut frame = FrameBuffer::new_white();
        let inside = native(OVERLAY_LEFT + 20, OVERLAY_TOP + 20);
        frame.set_native_black(inside, true);
        draw_wake_overlay(&mut frame).unwrap();
        assert_eq!(frame.is_black(inside), Some(false));
    }

    #[test]
    fn label_is_centered_within_the_box() {
        let mut frame = FrameBuffer::new_white();
        draw_wake_overlay(&mut frame).unwrap();
        let mut min_x = None;
        let mut max_x = None;
        for x in OVERLAY_LEFT..OVERLAY_LEFT + OVERLAY_WIDTH {
            if frame.is_black(native(x, OVERLAY_TOP + 40)) == Some(true) {
                min_x = min_x.or(Some(x));
                max_x = Some(x);
            }
        }
        let (min_x, max_x) = (min_x.unwrap(), max_x.unwrap());
        let left_margin = min_x - OVERLAY_LEFT;
        let right_margin = (OVERLAY_LEFT + OVERLAY_WIDTH) - max_x;
        assert!((left_margin - right_margin).abs() <= 4);
    }

    /// Host-only visual review aid, mirroring
    /// `app::mod::tests::export_screen_previews`: renders the overlay to a
    /// PNG in logical portrait orientation, as a human looks at the device,
    /// so the box placement and label can be eyeballed without physical
    /// hardware. Not part of the normal test run.
    #[test]
    #[ignore = "run explicitly with `cargo test -- --ignored` for a visual UI review"]
    fn export_wake_overlay_preview() {
        let mut frame = FrameBuffer::new_white();
        // A few marks standing in for whatever sleep-image content might be
        // underneath, so the preview also shows the box occluding it.
        let size = DisplayOrientation::Portrait.logical_size();
        for x in 0..size.width as i32 {
            frame.set_native_black(native(x, 0), true);
            frame.set_native_black(native(x, size.height as i32 - 1), true);
        }
        draw_wake_overlay(&mut frame).unwrap();

        let out_dir = std::env::temp_dir().join("rustmix-ui-preview");
        let _ = std::fs::create_dir_all(&out_dir);
        let mut canvas = image::GrayImage::new(size.width, size.height);
        for y in 0..size.height {
            for x in 0..size.width {
                let black = frame.is_black(native(x as i32, y as i32)).unwrap_or(false);
                canvas.put_pixel(x, y, image::Luma([if black { 0 } else { 255 }]));
            }
        }
        let path = out_dir.join("sleep-wake-overlay.png");
        canvas.save(&path).unwrap();
        println!("wrote preview to {}", path.display());
    }
}
