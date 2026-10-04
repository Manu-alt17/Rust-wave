//! Global Power-key menu: clean the screen, restart the device.

use core::convert::Infallible;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{draw_footer, select_and_back},
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE},
            list::{draw_list_row, draw_section_title, ROW_STEP},
            text::draw_paragraph,
        },
    },
    orientation::OrientedFrameBuffer,
};

pub fn render_power_key_menu(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    let body_line = i32::from(body.line_height());

    draw_header(display, state, t(locale, "POWER KEY", "TASTO POWER"))?;

    let baseline = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Screen cleaning", "Pulizia dello schermo"),
    )?;
    let after_text = draw_paragraph(
        display,
        t(
            locale,
            "A full refresh clears the ghosting left on the screen.",
            "Un aggiornamento completo elimina gli aloni rimasti sullo schermo.",
        ),
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        3,
        6,
    )?;

    let rows_top = after_text - body_line + 10;
    let rows = [
        t(locale, "Clear ghosting now", "Elimina aloni ora"),
        t(locale, "Restart", "Riavvia"),
        t(locale, "Cancel", "Annulla"),
    ];
    for (index, label) in rows.into_iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            rows_top + index as i32 * ROW_STEP,
            label,
            "",
            state.power_key_menu.selected == index,
        )?;
    }

    draw_paragraph(
        display,
        t(
            locale,
            "Power key: a short press switches the device off (standby, no battery used), a long press opens this menu.",
            "Tasto Power: la pressione breve spegne il dispositivo (standby, senza consumo), quella lunga apre questo menu.",
        ),
        CONTENT_LEFT,
        rows_top + rows.len() as i32 * ROW_STEP + 30,
        body,
        CONTENT_WIDTH,
        5,
        6,
    )?;

    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "RUN", "ESEGUI")),
    )
}

#[cfg(test)]
mod tests {
    use super::render_power_key_menu;
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn power_key_menu_renders_without_optional_services() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_power_key_menu(&mut display, &AppState::default()).unwrap();
    }
}
