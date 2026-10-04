//! RTC-backed Clock overview, readable details page and the runtime "Set
//! date & time" editor.

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::Text,
        widgets::{
            footer::{
                back_action, back_only, draw_footer, footer_hints, select_and_back, FooterKey,
            },
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{
                draw_field, draw_list_row, draw_row_frame, draw_section_title, ROW_PAD_X, ROW_STEP,
            },
            text::{draw_paragraph, draw_text_fit},
        },
    },
    clock_time_editor::ClockEditField,
    orientation::OrientedFrameBuffer,
    regional::Locale,
    rtc::RtcDateTime,
};

/// Height of the card holding the current time and date.
const TIME_CARD_HEIGHT: i32 = 124;

/// A date the way it is written in the user's language: "3 ottobre 2026",
/// "October 3, 2026".
fn long_date(locale: Locale, date: RtcDateTime) -> String {
    const ENGLISH: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    const ITALIAN: [&str; 12] = [
        "gennaio",
        "febbraio",
        "marzo",
        "aprile",
        "maggio",
        "giugno",
        "luglio",
        "agosto",
        "settembre",
        "ottobre",
        "novembre",
        "dicembre",
    ];
    let index = usize::from(date.month.clamp(1, 12)) - 1;
    match locale {
        Locale::English => format!("{} {}, {}", ENGLISH[index], date.day, date.year),
        Locale::Italian => format!("{} {} {}", date.day, ITALIAN[index], date.year),
    }
}

/// Draw the user-facing clock overview.
pub fn render_clock(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    let time = state.board.time_label(state.regional);
    let date = state.board.rtc.map_or_else(
        || t(locale, "Clock not available", "Orologio non disponibile").to_string(),
        |rtc| long_date(locale, state.regional.localize_rtc(rtc)),
    );
    let battery = state.battery_percent().map_or_else(
        || t(locale, "Not detected", "Non rilevata").to_string(),
        |percent| format!("{percent}%"),
    );

    draw_header(display, state, t(locale, "CLOCK", "OROLOGIO"))?;

    draw_row_frame(display, FIRST_ROW_TOP, TIME_CARD_HEIGHT, false)?;
    let card_left = CONTENT_LEFT + ROW_PAD_X;
    let card_width = CONTENT_WIDTH - 2 * ROW_PAD_X;
    Text::new(
        &time,
        Point::new(card_left, FIRST_ROW_TOP + 58),
        preferences.large_style(),
    )
    .draw(display)?;
    draw_text_fit(
        display,
        &date,
        Point::new(card_left, FIRST_ROW_TOP + 96),
        body,
        card_width,
    )?;

    let mut baseline = FIRST_ROW_TOP + TIME_CARD_HEIGHT + 44;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Battery", "Batteria"),
        &battery,
    )?;
    if let Some(power) = state.board.power {
        baseline = draw_field(
            display,
            preferences,
            baseline,
            t(locale, "USB cable", "Cavo USB"),
            if power.vbus_present {
                t(locale, "Connected", "Collegato")
            } else {
                t(locale, "Not connected", "Non collegato")
            },
        )?;
        baseline = draw_field(
            display,
            preferences,
            baseline,
            t(locale, "Charging", "Ricarica"),
            if power.charging {
                t(locale, "In progress", "In corso")
            } else {
                t(locale, "Not charging", "Non in carica")
            },
        )?;
    }

    let rows_top = baseline - i32::from(body.line_height()) + 10;
    let rows = [
        t(locale, "Set date and time", "Imposta data e ora"),
        t(locale, "Details", "Dettagli"),
    ];
    for (index, label) in rows.into_iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            rows_top + index as i32 * ROW_STEP,
            label,
            "",
            state.clock_action_selected == index,
        )?;
    }
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "OPEN", "APRI")),
    )
}

/// Draw the runtime-only local wall-clock editor opened from the Clock
/// overview's "Set date & time" action.
pub fn render_clock_set_time(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();

    draw_header(display, state, t(locale, "DATE & TIME", "DATA E ORA"))?;

    let Some(editor) = state.clock_time_editor.as_ref() else {
        draw_paragraph(
            display,
            t(locale, "No active edit.", "Nessuna modifica in corso."),
            CONTENT_LEFT,
            FIRST_BASELINE,
            body,
            CONTENT_WIDTH,
            2,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    };

    let hour = format!("{:02}", editor.draft.hour);
    let minute = format!("{:02}", editor.draft.minute);
    let year = format!("{:04}", editor.draft.year);
    let month = format!("{:02}", editor.draft.month);
    let day = format!("{:02}", editor.draft.day);
    let rows: [(&str, &str); 7] = [
        (
            t(locale, "Time zone", "Fuso orario"),
            editor.timezone.name(),
        ),
        (t(locale, "Day", "Giorno"), &day),
        (t(locale, "Month", "Mese"), &month),
        (t(locale, "Year", "Anno"), &year),
        (t(locale, "Hour", "Ora"), &hour),
        (t(locale, "Minute", "Minuti"), &minute),
        (t(locale, "Save", "Salva"), ""),
    ];
    for (index, (label, value)) in rows.into_iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            FIRST_ROW_TOP + index as i32 * ROW_STEP,
            label,
            value,
            editor.field_index == index,
        )?;
    }

    draw_paragraph(
        display,
        t(
            locale,
            "Nothing changes until you choose Save.",
            "Nulla cambia finch\u{00E9} non scegli Salva.",
        ),
        CONTENT_LEFT,
        FIRST_ROW_TOP + rows.len() as i32 * ROW_STEP + 26,
        body,
        CONTENT_WIDTH,
        3,
        6,
    )?;
    // BOOT steps back a field; on the first one it leaves without saving.
    let boot = if editor.field_index == 0 {
        t(locale, "CANCEL", "ANNULLA")
    } else {
        back_action(locale)
    };
    let hint = if editor.selected_field() == ClockEditField::Save {
        footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "SAVE", "SALVA")),
                (FooterKey::Boot, boot),
            ],
        )
    } else {
        footer_hints(
            locale,
            &[
                (FooterKey::UpDown, t(locale, "CHANGE", "CAMBIA")),
                (FooterKey::Select, t(locale, "NEXT", "AVANTI")),
                (FooterKey::Boot, boot),
            ],
        )
    };
    draw_footer(display, state, &hint)
}

/// Draw timezone, storage-basis and power details without crowding the overview.
pub fn render_clock_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let timezone = state.regional.timezone_label_for_rtc(state.board.rtc);
    let rtc_storage = state.regional.rtc_storage_label();
    let rtc_health = if state.board.rtc_clock_integrity_was_lost {
        t(locale, "Lost at startup", "Persa all'avvio")
    } else {
        t(locale, "Kept", "Mantenuta")
    };
    let unavailable = t(locale, "Unavailable", "Non disponibile");
    let battery_voltage = state
        .board
        .power
        .and_then(|power| power.battery_voltage_mv)
        .map_or_else(|| unavailable.to_string(), |mv| format!("{mv} mV"));

    draw_header(display, state, t(locale, "RTC INFO", "INFO RTC"))?;

    let mut baseline = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Clock", "Orologio"),
    )?;
    let clock: [(&str, &str); 3] = [
        (t(locale, "Time zone", "Fuso orario"), &timezone),
        (t(locale, "Stored as", "Ora memorizzata in"), &rtc_storage),
        (
            t(locale, "Time after power loss", "Ora dopo lo spegnimento"),
            rtc_health,
        ),
    ];
    for (label, value) in clock {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Power", "Alimentazione"),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Battery voltage", "Tensione batteria"),
        &battery_voltage,
    )?;
    if let Some(power) = state.board.power {
        baseline = draw_field(
            display,
            preferences,
            baseline,
            t(locale, "USB cable", "Cavo USB"),
            if power.vbus_present {
                t(locale, "Connected", "Collegato")
            } else {
                t(locale, "Not connected", "Non collegato")
            },
        )?;
        draw_field(
            display,
            preferences,
            baseline,
            t(locale, "Charging", "Ricarica"),
            if power.charging {
                t(locale, "In progress", "In corso")
            } else {
                t(locale, "Not charging", "Non in carica")
            },
        )?;
    }
    draw_footer(display, state, &back_only(locale))
}

#[cfg(test)]
mod tests {
    use super::{render_clock, render_clock_details, render_clock_set_time};
    use crate::{
        app::AppState, clock_time_editor::ClockTimeEditor, framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer, rtc::RtcDateTime,
    };

    #[test]
    fn clock_overview_and_details_render_without_optional_services() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_clock(&mut display, &state).unwrap();
        render_clock_details(&mut display, &state).unwrap();
    }

    #[test]
    fn clock_set_time_renders_with_and_without_an_active_editor() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        render_clock_set_time(&mut display, &state).unwrap();

        state.clock_time_editor = Some(ClockTimeEditor::new(
            RtcDateTime {
                year: 2026,
                month: 8,
                day: 18,
                weekday: 2,
                hour: 9,
                minute: 30,
                second: 0,
            },
            state.regional,
        ));
        render_clock_set_time(&mut display, &state).unwrap();
    }
}
