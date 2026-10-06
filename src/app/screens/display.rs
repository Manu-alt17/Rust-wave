//! Persistent global user-interface settings: text size, sleep screen and
//! automatic standby, and the chooser of the fixed sleep wallpaper.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
};

use crate::{
    app::{
        display::SleepScreenMode,
        i18n::t,
        state::{AppState, DISPLAY_FIXED_WALLPAPER_ROW},
        typography::Text,
        widgets::{
            footer::{
                back_action, back_only, draw_footer, draw_footer_paged, footer_hints,
                select_and_back, FooterKey,
            },
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP, SCREEN_WIDTH},
            list::{draw_list_row, ROW_STEP},
            text::draw_paragraph,
        },
    },
    orientation::OrientedFrameBuffer,
};

/// Top of the strip the chooser's footer takes from the wallpaper on show.
const PICKER_FOOTER_TOP: i32 = 738;
/// Logical portrait screen height.
const SCREEN_HEIGHT: i32 = 800;

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
                "What stays on the screen in standby: the wallpapers in turn, in order or at random, always the same one (Fixed), or the cover of the book being read.",
                "Cosa resta sullo schermo in standby: gli sfondi a rotazione, in ordine o a caso, sempre lo stesso (Fissa), oppure la copertina del libro in lettura.",
            ),
        ),
        (
            t(locale, "Fixed wallpaper", "Sfondo fisso"),
            t(locale, "Choose", "Scegli"),
            t(
                locale,
                "Look at the wallpapers one at a time and choose the one that stays in standby. Choosing one sets the sleep screen to Fixed.",
                "Guarda gli sfondi uno alla volta e scegli quello che resta in standby. Sceglierne uno imposta la schermata di standby su Fissa.",
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

    let action = if state.display_action_selected == DISPLAY_FIXED_WALLPAPER_ROW {
        t(locale, "OPEN", "APRI")
    } else {
        t(locale, "CHANGE", "CAMBIA")
    };
    draw_footer(display, state, &select_and_back(locale, action))
}

/// Draw the chooser of the fixed sleep wallpaper: the wallpaper under the
/// cursor exactly as standby will show it, over the whole panel, with only
/// the footer's strip taken from it. Without a wallpaper to show, an
/// ordinary page says why.
pub fn render_sleep_picker(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let picker = &state.sleep_picker;

    let Some(frame) = picker.preview() else {
        draw_header(display, state, t(locale, "FIXED WALLPAPER", "SFONDO FISSO"))?;
        let message = if picker.is_empty() {
            t(
                locale,
                "There are no wallpapers on the card yet. Add one from the Wi-Fi page: Upload, Wi-Fi, then Wallpapers on the phone.",
                "Sulla scheda non ci sono ancora sfondi. Aggiungine uno dalla pagina Wi-Fi: Carica, Wi-Fi, poi Sfondi sul telefono.",
            )
        } else {
            t(
                locale,
                "This wallpaper cannot be read. Move on to the next one.",
                "Questo sfondo non si riesce a leggere. Passa al successivo.",
            )
        };
        draw_paragraph(
            display,
            message,
            CONTENT_LEFT,
            FIRST_BASELINE,
            preferences.body_style(),
            CONTENT_WIDTH,
            6,
            6,
        )?;
        let hint = if picker.is_empty() {
            back_only(locale)
        } else {
            footer_hints(
                locale,
                &[
                    (FooterKey::UpDown, t(locale, "NEXT", "CAMBIA")),
                    (FooterKey::Boot, back_action(locale)),
                ],
            )
        };
        return draw_footer_paged(display, state, &hint, picker.position());
    };

    display.copy_native_frame(frame);
    let white = PrimitiveStyle::with_fill(BinaryColor::Off);
    Rectangle::new(
        Point::new(0, PICKER_FOOTER_TOP),
        Size::new(
            SCREEN_WIDTH as u32,
            (SCREEN_HEIGHT - PICKER_FOOTER_TOP) as u32,
        ),
    )
    .into_styled(white)
    .draw(display)?;

    // The one standby shows now is said on the picture, in a label of its
    // own: over a dithered image bare text cannot be read.
    if preferences.sleep_screen == SleepScreenMode::Fixed && picker.selected_is_current() {
        let style = preferences.body_style();
        let label = t(locale, "Current fixed wallpaper", "Sfondo fisso attuale");
        let line_height = i32::from(style.line_height());
        let box_style = PrimitiveStyleBuilder::new()
            .fill_color(BinaryColor::Off)
            .stroke_color(BinaryColor::On)
            .stroke_width(1)
            .build();
        Rectangle::new(
            Point::new(14, 14),
            Size::new(
                (style.text_width(label) + 24) as u32,
                (line_height + 16) as u32,
            ),
        )
        .into_styled(box_style)
        .draw(display)?;
        Text::new(label, Point::new(26, 14 + 8 + line_height - 4), style).draw(display)?;
    }

    draw_footer_paged(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "USE", "USA")),
                (FooterKey::UpDown, t(locale, "NEXT", "CAMBIA")),
                (FooterKey::Boot, back_action(locale)),
            ],
        ),
        picker.position(),
    )
}
