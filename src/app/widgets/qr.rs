//! QR code payload builder and e-paper rendering for the phone Wi-Fi
//! provisioning portal: a single code that auto-joins a phone to the
//! device's hotspot. The captive-portal DNS hijack in
//! `crate::dns_captive_portal` then gets the phone's OS to open the portal
//! on its own, so no second "open this URL" QR code is needed.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
    Drawable,
};
use qrcode::{Color, QrCode};

use crate::orientation::OrientedFrameBuffer;

/// Build the de-facto `WIFI:` QR payload that iOS and Android camera apps
/// recognize and offer to join automatically, escaping the characters the
/// format reserves as separators.
#[must_use]
pub fn wifi_join_payload(ssid: &str, password: &str) -> String {
    if password.is_empty() {
        format!("WIFI:T:nopass;S:{};;", escape_wifi_qr_field(ssid))
    } else {
        format!(
            "WIFI:T:WPA;S:{};P:{};;",
            escape_wifi_qr_field(ssid),
            escape_wifi_qr_field(password)
        )
    }
}

/// Escape `;`, `,`, `:`, `\` and `"` with a backslash, per the Wi-Fi QR
/// payload convention shared by iOS and Android.
fn escape_wifi_qr_field(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, ';' | ',' | ':' | '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Number of QR modules per side needed to encode `payload`, or `None` if it
/// could not be encoded. Lets a caller center a code (by computing its final
/// pixel width as `modules * module_px`) before committing to a `top_left`
/// for [`draw_qr`], which only reports the size after it has already drawn.
#[must_use]
pub fn qr_modules_wide(payload: &str) -> Option<u32> {
    QrCode::new(payload.as_bytes())
        .ok()
        .map(|code| code.width() as u32)
}

/// Render `payload` as a QR code on the e-paper display, with each module
/// drawn as a `module_px`-sized filled square starting at `top_left`.
/// Returns the rendered side length in pixels so callers can lay out
/// surrounding labels, or `None` if the payload could not be encoded (too
/// long for a QR code, which never happens for the fixed-shape Wi-Fi/URL
/// payloads this module builds).
pub fn draw_qr(
    display: &mut OrientedFrameBuffer<'_>,
    top_left: Point,
    module_px: i32,
    payload: &str,
) -> Result<Option<u32>, Infallible> {
    let Ok(code) = QrCode::new(payload.as_bytes()) else {
        return Ok(None);
    };
    let width = code.width();
    let colors = code.to_colors();
    let dark_style = PrimitiveStyle::with_fill(BinaryColor::On);
    for row in 0..width {
        for column in 0..width {
            if colors[row * width + column] != Color::Dark {
                continue;
            }
            let point = Point::new(
                top_left.x + (column as i32) * module_px,
                top_left.y + (row as i32) * module_px,
            );
            Rectangle::new(point, Size::new(module_px as u32, module_px as u32))
                .into_styled(dark_style)
                .draw(display)?;
        }
    }
    Ok(Some((width as i32 * module_px) as u32))
}

#[cfg(test)]
mod tests {
    use super::{escape_wifi_qr_field, qr_modules_wide, wifi_join_payload};

    #[test]
    fn wpa_payload_includes_ssid_and_password() {
        assert_eq!(
            wifi_join_payload("Rustmix", "correct-horse"),
            "WIFI:T:WPA;S:Rustmix;P:correct-horse;;"
        );
    }

    #[test]
    fn open_network_payload_uses_nopass() {
        assert_eq!(wifi_join_payload("Guest", ""), "WIFI:T:nopass;S:Guest;;");
    }

    #[test]
    fn reserved_characters_are_escaped() {
        assert_eq!(escape_wifi_qr_field("a;b,c:d\\e\"f"), "a\\;b\\,c\\:d\\\\e\\\"f");
    }

    #[test]
    fn qr_modules_wide_matches_the_side_length_draw_qr_reports() {
        let payload = wifi_join_payload("Rustmix", "correct-horse");
        let modules = qr_modules_wide(&payload).unwrap();

        use crate::{framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};
        use embedded_graphics::prelude::Point;
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let side = super::draw_qr(&mut display, Point::new(10, 10), 4, &payload)
            .unwrap()
            .unwrap();
        assert_eq!(side, modules * 4);
    }

    #[test]
    fn draw_qr_reports_rendered_side_length_in_pixels() {
        use crate::{framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};
        use embedded_graphics::prelude::Point;

        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let side = super::draw_qr(&mut display, Point::new(10, 10), 4, "http://192.168.71.1/")
            .unwrap()
            .unwrap();
        assert!(side > 0);
    }
}
