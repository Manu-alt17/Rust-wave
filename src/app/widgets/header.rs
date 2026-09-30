//! Reusable black product header.

use core::convert::Infallible;

use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextRole},
        widgets::status_glyphs::draw_status_group,
    },
    orientation::OrientedFrameBuffer,
};

/// Height of the header: a single row holding the persistent Wi-Fi /
/// battery status on the left, the screen title centered, and the clock on
/// the right, all on one baseline. No divider line below it.
pub const HEADER_TOTAL_HEIGHT: u32 = 44;

/// Left margin of the Wi-Fi / battery status group.
const STATUS_LEFT: i32 = 18;
/// Rightmost inked pixel of the clock.
const TIME_RIGHT: i32 = 462;
/// Logical portrait screen width, used to center the title between the
/// status group and the clock.
const SCREEN_WIDTH: i32 = 480;

/// Draw the product header shared by every portrait screen: the persistent
/// Wi-Fi / battery status on the left, the screen title centered, and the
/// clock on the right, all on one baseline, in black ink on the white
/// background.
pub fn draw_header(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    title: &str,
) -> Result<(), Infallible> {
    let preferences = state.display;
    let title_style = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
    let baseline_y = 30;

    let status_right = draw_status_group(
        display,
        preferences,
        Point::new(STATUS_LEFT, baseline_y),
        state.wifi_connected(),
        state.battery_percent(),
        state.battery_charging(),
        BinaryColor::On,
    )?;

    let time_label = state.status_time_label();
    let time_x = TIME_RIGHT - title_style.text_width(&time_label);
    Text::new(&time_label, Point::new(time_x, baseline_y), title_style).draw(display)?;

    let max_title_width = centered_title_budget(status_right, time_x);
    let title_style = if title_style.text_width(title) <= max_title_width {
        title_style
    } else {
        // Safety net for a title (or a future/dynamic one) that is still too
        // wide at the chosen font profile: drop one text tier rather than
        // printing over the battery percentage or the clock.
        preferences.body_style()
    };
    let title_x = (SCREEN_WIDTH - title_style.text_width(title)) / 2;
    Text::new(title, Point::new(title_x, baseline_y), title_style).draw(display)?;
    Ok(())
}

/// Minimum clear gap kept between the centered title and its neighbours.
const TITLE_SIDE_GAP: i32 = 8;

/// Widest title that, centered on the screen, stays `TITLE_SIDE_GAP` clear
/// of both the status group ending at `status_right` and the clock starting
/// at `time_x`.
const fn centered_title_budget(status_right: i32, time_x: i32) -> i32 {
    let center = SCREEN_WIDTH / 2;
    let left_room = center - status_right;
    let right_room = time_x - center;
    let half = if left_room < right_room {
        left_room
    } else {
        right_room
    };
    2 * (half - TITLE_SIDE_GAP)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;

    use super::{centered_title_budget, STATUS_LEFT, TIME_RIGHT};
    use crate::{
        app::{
            display::{DisplayPreferences, UiFontFamily, UiFontSize},
            typography::UiTextRole,
        },
        reader::ReadingPreference,
        regional::Locale,
    };

    /// Every fixed header title must fit at full Heading size between the
    /// battery status and the clock in the worst case: largest font
    /// profile, charging glyph shown, "100%". (`draw_header` would otherwise
    /// fall back to the smaller Body tier.) Add new titles here.
    #[test]
    fn header_titles_fit_between_status_and_clock() {
        #[rustfmt::skip]
        const TITLES: &[&str] = &[
            "ALARMS", "SVEGLIE", "AUDIO INFO", "INFO AUDIO",
            "CLOCK", "OROLOGIO",
            "RTC INFO", "INFO RTC", "DATE & TIME", "DATA E ORA", "DICTIONARY", "DIZIONARIO",
            "DISPLAY", "SCHERMO", "ENVIRONMENT", "AMBIENTE", "SENSOR INFO", "INFO SENSORE",
            "FILE PREVIEW", "ANTEPRIMA", "LANGUAGE", "LINGUA",
            "MOTION",
            "MOVIMENTO", "EVENTS", "EVENTI", "MOTION INFO", "INFO IMU", "NETWORK", "RETE",
            "NETWORK INFO", "INFO RETE", "SAVED WI-FI", "RETI SALVATE", "UPLOAD", "CARICA",
            "UPDATE", "POWER KEY", "TASTO POWER", "BOOK OPTIONS", "OPZIONI LIBRO",
            "BOOKMARKS", "SEGNALIBRI", "CONTINUE", "CONTINUA", "LIBRARY", "LIBRERIA",
            "OPENING BOOK", "APERTURA", "OPTIONS", "OPZIONI", "PREFERENCES", "PREFERENZE",
            "CONTENTS", "INDICE", "STATISTICS", "STATISTICHE", "RECORD", "REGISTRA",
            "VOICE NOTE", "NOTA VOCALE", "NOTE TITLE", "TITOLO NOTA", "VOICE NOTES",
            "NOTE VOCALI",
        ];
        let mut titles: Vec<&str> = TITLES.to_vec();
        for preference in ReadingPreference::ALL {
            titles.push(preference.header_label_i18n(Locale::English));
            titles.push(preference.header_label_i18n(Locale::Italian));
        }
        for font_family in [UiFontFamily::Inter, UiFontFamily::AtkinsonHyperlegible] {
            for font_size in [UiFontSize::Compact, UiFontSize::Standard, UiFontSize::Large] {
                let preferences = DisplayPreferences {
                    font_family,
                    font_size,
                    ..DisplayPreferences::default()
                };
                let style = preferences.text_style(UiTextRole::Heading, BinaryColor::On);
                // Wi-Fi icon + gap + charging icon + gap + "100%".
                let status_right = STATUS_LEFT + 24 + 6 + 24 + 6 + style.text_width("100%");
                let time_x = TIME_RIGHT - style.text_width("00:00");
                let budget = centered_title_budget(status_right, time_x);
                for title in &titles {
                    assert!(
                        style.text_width(title) <= budget,
                        "{title:?} is {}px, budget {budget}px at {font_family:?}/{font_size:?}",
                        style.text_width(title),
                    );
                }
            }
        }
    }
}
