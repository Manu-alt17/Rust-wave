//! Reusable footer hints for button-driven screens.
//!
//! One line under a separator tells which keys do something on the screen:
//! `SELECT APRI · BOOT INDIETRO`. Every screen builds it with
//! [`footer_hints`], so the wording, the order and the separator are the
//! same everywhere; a list shown a page at a time adds its page on the
//! right with [`draw_footer_paged`].

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::text::truncate_to_width,
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

/// Left edge of the hint text.
const FOOTER_LEFT: i32 = 18;
/// Right edge of the hint text and of the page indicator.
const FOOTER_RIGHT: i32 = 462;
/// Baseline of the hint text.
const FOOTER_BASELINE: i32 = 782;
/// Gap kept between the hint and the page indicator.
const FOOTER_PAGE_GAP: i32 = 16;
/// What joins a hint's actions.
const FOOTER_SEPARATOR: &str = " \u{00B7} ";

/// A physical control a footer hint can name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FooterKey {
    /// Rocker press.
    Select,
    /// Rocker press held.
    Hold,
    /// Rocker up or down.
    UpDown,
    /// BOOT button.
    Boot,
}

impl FooterKey {
    const fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Hold => t(locale, "HOLD", "TIENI"),
            Self::UpDown => t(locale, "UP/DOWN", "SU/GI\u{00D9}"),
            Self::Boot => "BOOT",
        }
    }
}

/// `SELECT APRI · BOOT INDIETRO` from `(key, action)` pairs, the actions
/// already in the user's language.
#[must_use]
pub fn footer_hints(locale: Locale, parts: &[(FooterKey, &str)]) -> String {
    parts
        .iter()
        .map(|(key, action)| format!("{} {action}", key.label(locale)))
        .collect::<Vec<_>>()
        .join(FOOTER_SEPARATOR)
}

/// The hint every screen below Home ends with.
#[must_use]
pub const fn back_action(locale: Locale) -> &'static str {
    t(locale, "BACK", "INDIETRO")
}

/// `SELECT <action> · BOOT INDIETRO`, the most common footer.
#[must_use]
pub fn select_and_back(locale: Locale, action: &str) -> String {
    footer_hints(
        locale,
        &[
            (FooterKey::Select, action),
            (FooterKey::Boot, back_action(locale)),
        ],
    )
}

/// `BOOT INDIETRO`, for screens with nothing to select.
#[must_use]
pub fn back_only(locale: Locale) -> String {
    footer_hints(locale, &[(FooterKey::Boot, back_action(locale))])
}

/// Draw the footer separator and the one-line button hint.
pub fn draw_footer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    hint: &str,
) -> Result<(), Infallible> {
    draw_footer_paged(display, state, hint, None)
}

/// `hint` as it fits in `available` pixels. A hint is its actions in order
/// of use, joined by [`FOOTER_SEPARATOR`], with the way back last; one too
/// wide for the line loses whole actions from the end (never the first, the
/// one this screen is for), and only a single action that is still too wide
/// is cut. There is no smaller text to fall back on.
fn fit_hint(style: UiTextStyle, hint: &str, available: i32) -> String {
    let mut parts: Vec<&str> = hint.split(FOOTER_SEPARATOR).collect();
    while parts.len() > 1 && style.text_width(&parts.join(FOOTER_SEPARATOR)) > available {
        parts.pop();
    }
    truncate_to_width(style, &parts.join(FOOTER_SEPARATOR), available)
}

/// Draw the footer with `page / pages` on the right. The indicator is left
/// out for a single page. A hint too wide for the line is shortened by
/// [`fit_hint`].
pub fn draw_footer_paged(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    hint: &str,
    page: Option<(usize, usize)>,
) -> Result<(), Infallible> {
    let preferences = state.display;
    let line = PrimitiveStyle::with_fill(BinaryColor::On);
    Rectangle::new(Point::new(14, 746), Size::new(452, 1))
        .into_styled(line)
        .draw(display)?;

    let mut hint_right = FOOTER_RIGHT;
    if let Some((current, pages)) = page.filter(|(_, pages)| *pages > 1) {
        let style = preferences.footer_style();
        let label = format!("{current}/{pages}");
        let left = FOOTER_RIGHT - style.text_width(&label);
        Text::new(&label, Point::new(left, FOOTER_BASELINE), style).draw(display)?;
        hint_right = left - FOOTER_PAGE_GAP;
    }

    let available = hint_right - FOOTER_LEFT;
    let style = preferences.footer_style();
    Text::new(
        &fit_hint(style, hint, available),
        Point::new(FOOTER_LEFT, FOOTER_BASELINE),
        style,
    )
    .draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{back_only, fit_hint, footer_hints, select_and_back, FooterKey};
    use crate::{
        app::display::{DisplayPreferences, UiFontSize},
        regional::Locale,
    };

    #[test]
    fn a_hint_too_wide_loses_whole_actions_from_the_end() {
        let style = DisplayPreferences {
            font_size: UiFontSize::Large,
            ..Default::default()
        }
        .footer_style();
        let hint = "SU/GI\u{00D9} CAMBIA \u{00B7} SELECT AVANTI \u{00B7} BOOT INDIETRO";
        let full = style.text_width(hint);
        // Wide enough: untouched.
        assert_eq!(fit_hint(style, hint, full), hint);
        // One pixel short: the way back goes, whole, and nothing is cut.
        assert_eq!(
            fit_hint(style, hint, full - 1),
            "SU/GI\u{00D9} CAMBIA \u{00B7} SELECT AVANTI"
        );
        // The first action always stays, cut if it must be.
        let first = style.text_width("SU/GI\u{00D9} CAMBIA");
        assert_eq!(fit_hint(style, hint, first), "SU/GI\u{00D9} CAMBIA");
        let cut = fit_hint(style, hint, first - 1);
        assert!(
            cut.starts_with("SU/GI") && cut.ends_with('\u{2026}'),
            "{cut}"
        );
        assert!(style.text_width(&cut) < first);
    }

    #[test]
    fn hints_use_one_wording_and_one_separator() {
        assert_eq!(
            select_and_back(Locale::Italian, "APRI"),
            "SELECT APRI \u{00B7} BOOT INDIETRO"
        );
        assert_eq!(back_only(Locale::English), "BOOT BACK");
        assert_eq!(
            footer_hints(
                Locale::Italian,
                &[(FooterKey::UpDown, "VOLUME"), (FooterKey::Hold, "MENU")]
            ),
            "SU/GI\u{00D9} VOLUME \u{00B7} TIENI MENU"
        );
    }
}
