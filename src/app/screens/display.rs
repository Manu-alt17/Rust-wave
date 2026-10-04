//! Persistent global user-interface settings: text size, sleep screen and
//! automatic standby.

use core::convert::Infallible;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{draw_footer, select_and_back},
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_ROW_TOP},
            list::{draw_list_row, ROW_STEP},
            text::draw_paragraph,
        },
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_display(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;

    draw_header(display, state, t(locale, "DISPLAY", "SCHERMO"))?;

    let rows = [
        (
            t(locale, "Text size", "Dimensione testo"),
            preferences.font_size.label_i18n(locale),
            t(
                locale,
                "Size of the text in menus and settings. Book text is set in the reading preferences.",
                "Dimensione dei testi di menu e impostazioni. Il testo dei libri si regola nelle preferenze di lettura.",
            ),
        ),
        (
            t(locale, "Sleep screen", "Schermata di standby"),
            preferences.sleep_screen.label_i18n(locale),
            t(
                locale,
                "What stays on the screen in standby: the images in the SLEEP folder, in order or at random, or the cover of the book being read.",
                "Cosa resta sullo schermo in standby: le immagini della cartella SLEEP, in ordine o a caso, oppure la copertina del libro in lettura.",
            ),
        ),
        (
            t(locale, "Auto standby", "Standby automatico"),
            preferences.auto_sleep.label_i18n(locale),
            t(
                locale,
                "How long without a key press before the device goes to standby by itself.",
                "Dopo quanto tempo senza premere tasti il dispositivo va in standby da solo.",
            ),
        ),
    ];
    for (index, (label, value, _)) in rows.iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            FIRST_ROW_TOP + index as i32 * ROW_STEP,
            label,
            value,
            state.display_action_selected == index,
        )?;
    }
    // The selected row explained: the labels alone are short.
    let help = rows
        .get(state.display_action_selected)
        .map_or("", |(_, _, help)| help);
    draw_paragraph(
        display,
        help,
        CONTENT_LEFT,
        FIRST_ROW_TOP + rows.len() as i32 * ROW_STEP + 30,
        preferences.body_style(),
        CONTENT_WIDTH,
        6,
        6,
    )?;

    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "CHANGE", "CAMBIA")),
    )
}
