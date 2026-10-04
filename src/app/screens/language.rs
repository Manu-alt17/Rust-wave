//! On-device language selection.

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

pub fn render_language(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;

    draw_header(display, state, t(locale, "LANGUAGE", "LINGUA"))?;
    draw_list_row(
        display,
        preferences,
        FIRST_ROW_TOP,
        t(locale, "Language", "Lingua"),
        locale.display_label(),
        true,
    )?;
    draw_paragraph(
        display,
        t(
            locale,
            "The language of menus and messages. Books stay in their own language.",
            "La lingua di menu e messaggi. I libri restano nella loro lingua.",
        ),
        CONTENT_LEFT,
        FIRST_ROW_TOP + ROW_STEP + 30,
        preferences.body_style(),
        CONTENT_WIDTH,
        4,
        6,
    )?;
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "CHANGE", "CAMBIA")),
    )
}
