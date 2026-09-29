//! Native RTC-localized Calendar with X4-compatible U.S. and personal events.

use core::convert::Infallible;
use std::collections::BTreeSet;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextRole, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    calendar::{
        compact_text, days_in_month, weekday, CalendarDate, CalendarEvent, CalendarEventKind,
        CALENDAR_EDITOR_KEY_ROWS,
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

const GRID_LEFT: i32 = 26;
const GRID_TOP: i32 = 218;
const CELL_WIDTH: i32 = 61;
const CELL_HEIGHT: i32 = 48;
const AGENDA_SUMMARY_TOP: i32 = 144;
const AGENDA_SUMMARY_HEIGHT: u32 = 94;
const AGENDA_STATUS_BASELINE: i32 = 172;
const AGENDA_NOTICE_BASELINE: i32 = 196;
const AGENDA_RANGE_BASELINE: i32 = 220;
const AGENDA_FIRST_ROW_TOP: i32 = 254;
const AGENDA_ROW_STEP: i32 = 60;
const AGENDA_ROW_HEIGHT: u32 = 54;
const AGENDA_FOOTER_HINT_EN: &str = "HOLD ADD";
const AGENDA_FOOTER_HINT_IT: &str = "TIENI AGGIUNGI";
const CALENDAR_EDITOR_FOOTER_HINT_EN: &str = "HOLD H/V  SELECT KEY";
const CALENDAR_EDITOR_FOOTER_HINT_IT: &str = "TIENI H/V  SELECT TASTO";
const WEEKDAY_LABELS_EN: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
const WEEKDAY_LABELS_IT: [&str; 7] = ["DOM", "LUN", "MAR", "MER", "GIO", "VEN", "SAB"];
const MONTH_LABELS_EN: [&str; 12] = [
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
const MONTH_LABELS_IT: [&str; 12] = [
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

fn weekday_labels(locale: Locale) -> &'static [&'static str; 7] {
    match locale {
        Locale::English => &WEEKDAY_LABELS_EN,
        Locale::Italian => &WEEKDAY_LABELS_IT,
    }
}

/// Render the RTC-localized monthly Calendar page with compact event markers.
pub fn render_calendar(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let cursor = state.calendar.cursor;
    let month = month_label(locale, cursor.month);
    let month_year = format!("{month} {}", cursor.year);
    let selected = selected_date_label(locale, cursor);
    let today = state
        .board
        .rtc
        .map(|rtc| CalendarDate::from_rtc(state.regional.localize_rtc(rtc)));

    draw_header(display, state, t(locale, "CALENDAR", "CALENDARIO"))?;

    Text::new(
        &month_year,
        Point::new(24, 126),
        state.display.heading_style(),
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "SELECT changes DAY / MONTH navigation.",
            "SELECT cambia navigazione GIORNO / MESE.",
        ),
        Point::new(24, 158),
        state.display.body_style(),
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "Hold SELECT opens selected-day agenda.",
            "Tieni premuto SELECT per l'agenda del giorno.",
        ),
        Point::new(24, 184),
        state.display.detail_style(),
    )
    .draw(display)?;

    for (column, label) in weekday_labels(locale).iter().enumerate() {
        Text::new(
            label,
            Point::new(GRID_LEFT + column as i32 * CELL_WIDTH + 8, 206),
            state.display.detail_style(),
        )
        .draw(display)?;
    }

    draw_month_grid(display, state, cursor, today)?;

    Rectangle::new(Point::new(22, 538), Size::new(436, 112))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        t(locale, "Selected day", "Giorno selezionato"),
        Point::new(40, 568),
        state.display.body_style(),
    )
    .draw(display)?;
    Text::new(
        &selected,
        Point::new(40, 602),
        state.display.heading_style(),
    )
    .draw(display)?;
    Text::new(
        &state.calendar.selected_day_summary(),
        Point::new(40, 634),
        state.display.body_style(),
    )
    .draw(display)?;
    Text::new(
        &state.calendar.catalog.status_label(),
        Point::new(24, 670),
        state.display.detail_style(),
    )
    .draw(display)?;

    draw_footer(
        display,
        state,
        t(
            locale,
            "SELECT MODE  HOLD AGENDA",
            "SELECT MODALITÀ  TIENI AGENDA",
        ),
    )?;
    Ok(())
}

/// Render the selected-day scrollable agenda.
pub fn render_calendar_agenda(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let date = selected_date_label(locale, state.calendar.cursor);
    let events = state.calendar.selected_day_events();
    draw_header(display, state, t(locale, "CALENDAR", "CALENDARIO"))?;
    Text::new(&date, Point::new(22, 126), state.display.heading_style()).draw(display)?;
    Rectangle::new(
        Point::new(22, AGENDA_SUMMARY_TOP),
        Size::new(436, AGENDA_SUMMARY_HEIGHT),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(display)?;
    Text::new(
        &compact_text(&state.calendar.catalog.status_label(), 54),
        Point::new(34, AGENDA_STATUS_BASELINE),
        state.display.detail_style(),
    )
    .draw(display)?;
    Text::new(
        &compact_text(&state.calendar.notice, 54),
        Point::new(34, AGENDA_NOTICE_BASELINE),
        state.display.detail_style(),
    )
    .draw(display)?;
    let visible = state.calendar.agenda_visible_range_for_len(events.len());
    Text::new(
        &agenda_visible_range_label(locale, events.len(), &visible),
        Point::new(34, AGENDA_RANGE_BASELINE),
        state.display.detail_style(),
    )
    .draw(display)?;

    if events.is_empty() {
        Rectangle::new(Point::new(22, AGENDA_FIRST_ROW_TOP), Size::new(436, 118))
            .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
            .draw(display)?;
        Text::new(
            t(
                locale,
                "No events for selected day.",
                "Nessun evento per il giorno selezionato.",
            ),
            Point::new(44, AGENDA_FIRST_ROW_TOP + 58),
            state.display.body_style(),
        )
        .draw(display)?;
    } else {
        for (visible_index, event_index) in visible.enumerate() {
            let event = events[event_index];
            let selected = event_index == state.calendar.agenda_selected;
            draw_agenda_row(
                display,
                state,
                AGENDA_FIRST_ROW_TOP + visible_index as i32 * AGENDA_ROW_STEP,
                event,
                selected,
            )?;
        }
    }

    draw_footer(
        display,
        state,
        t(locale, AGENDA_FOOTER_HINT_EN, AGENDA_FOOTER_HINT_IT),
    )?;
    Ok(())
}

/// Render one personal or read-only U.S. calendar event.
pub fn render_calendar_event_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let Some(event) = state.calendar.selected_agenda_event() else {
        draw_header(
            display,
            state,
            t(locale, "EVENT", "EVENTO"),
        )?;
        Text::new(
            t(
                locale,
                "No event is selected.",
                "Nessun evento selezionato.",
            ),
            Point::new(22, 184),
            state.display.body_style(),
        )
        .draw(display)?;
        return Ok(());
    };

    let personal = event.kind == CalendarEventKind::Personal;
    draw_header(
        display,
        state,
        t(locale, "EVENT", "EVENTO"),
    )?;
    let date = selected_date_label(locale, event.date);
    Text::new(
        &compact_text(&event.title, 34),
        Point::new(22, 130),
        state.display.heading_style(),
    )
    .draw(display)?;
    detail_line(
        display,
        184,
        t(locale, "Source", "Origine"),
        event.kind.source_file(),
        state.display.body_style(),
    )?;
    detail_line(
        display,
        228,
        t(locale, "Category", "Categoria"),
        event.kind.label(),
        state.display.body_style(),
    )?;
    detail_line(
        display,
        272,
        t(locale, "Date", "Data"),
        &date,
        state.display.body_style(),
    )?;
    Rectangle::new(Point::new(22, 304), Size::new(436, 148))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        t(locale, "Detail", "Dettaglio"),
        Point::new(40, 340),
        state.display.body_style(),
    )
    .draw(display)?;
    draw_wrapped_detail(display, state, &event.detail, 40, 378)?;

    if personal {
        for (index, label) in [
            t(locale, "Edit personal event", "Modifica evento personale"),
            t(locale, "Delete personal event", "Elimina evento personale"),
            t(locale, "Return to agenda", "Torna all'agenda"),
        ]
        .iter()
        .enumerate()
        {
            draw_action_row(
                display,
                state,
                480 + index as i32 * 54,
                label,
                index == state.calendar.details_action_selected,
            )?;
        }
        Text::new(
            t(
                locale,
                "Only personal EVENTS.TXT rows can change.",
                "Solo le righe personali di EVENTS.TXT possono cambiare.",
            ),
            Point::new(22, 668),
            state.display.detail_style(),
        )
        .draw(display)?;
        draw_footer(display, state, t(locale, "SELECT ACTION", "SELECT AZIONE"))?;
    } else {
        Text::new(
            t(
                locale,
                "U.S. pack entries remain read-only.",
                "Le voci del pacchetto USA restano di sola lettura.",
            ),
            Point::new(22, 526),
            state.display.body_style(),
        )
        .draw(display)?;
    }
    Ok(())
}

/// Render create/edit personal-event keyboard screen.
pub fn render_calendar_event_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let Some(editor) = state.calendar.editor.as_ref() else {
        draw_header(
            display,
            state,
            t(locale, "EDIT EVENT", "MODIFICA"),
        )?;
        Text::new(
            t(
                locale,
                "No personal event editor is active.",
                "Nessun editor evento personale attivo.",
            ),
            Point::new(22, 174),
            state.display.body_style(),
        )
        .draw(display)?;
        return Ok(());
    };
    draw_header(
        display,
        state,
        t(locale, "EDIT EVENT", "MODIFICA"),
    )?;
    Text::new(
        t(locale, "Title", "Titolo"),
        Point::new(22, 124),
        state.display.body_style(),
    )
    .draw(display)?;
    Rectangle::new(Point::new(22, 138), Size::new(436, 46))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        &truncate_or_placeholder(&editor.title, "_", 44),
        Point::new(34, 168),
        state.display.body_style(),
    )
    .draw(display)?;
    Text::new(
        t(locale, "Detail", "Dettaglio"),
        Point::new(22, 214),
        state.display.body_style(),
    )
    .draw(display)?;
    Rectangle::new(Point::new(22, 228), Size::new(436, 58))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    Text::new(
        &truncate_or_placeholder(&editor.detail, "_", 50),
        Point::new(34, 264),
        state.display.detail_style(),
    )
    .draw(display)?;
    Text::new(
        &compact_text(&editor.message, 56),
        Point::new(22, 314),
        state.display.detail_style(),
    )
    .draw(display)?;
    draw_editor_keyboard(display, state)?;
    draw_footer(
        display,
        state,
        t(
            locale,
            CALENDAR_EDITOR_FOOTER_HINT_EN,
            CALENDAR_EDITOR_FOOTER_HINT_IT,
        ),
    )?;
    Ok(())
}

/// Render explicit delete confirmation for one personal row.
pub fn render_calendar_delete_confirmation(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_header(
        display,
        state,
        t(locale, "DELETE?", "ELIMINA?"),
    )?;
    let title = state.calendar.selected_agenda_event().map_or(
        t(locale, "No event selected", "Nessun evento selezionato"),
        |event| event.title.as_str(),
    );
    Text::new(
        &compact_text(title, 36),
        Point::new(22, 160),
        state.display.heading_style(),
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "The U.S. holiday pack is never modified.",
            "Il pacchetto festività USA non viene mai modificato.",
        ),
        Point::new(22, 208),
        state.display.body_style(),
    )
    .draw(display)?;
    for (index, label) in [
        t(locale, "Cancel", "Annulla"),
        t(locale, "Delete permanently", "Elimina definitivamente"),
    ]
    .iter()
    .enumerate()
    {
        draw_action_row(
            display,
            state,
            286 + index as i32 * 64,
            label,
            index == state.calendar.delete_confirmation_selected,
        )?;
    }
    Ok(())
}

fn draw_month_grid(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    cursor: CalendarDate,
    today: Option<CalendarDate>,
) -> Result<(), Infallible> {
    let first_weekday = usize::from(weekday(cursor.year, cursor.month, 1));
    let month_days = days_in_month(cursor.year, cursor.month);
    // One pass over the whole catalog instead of one per day cell (up to
    // 31 full linear scans of up to `CALENDAR_EVENT_LIMIT` events each).
    let days_with_events: BTreeSet<CalendarDate> = state
        .calendar
        .catalog
        .events
        .iter()
        .filter(|event| event.date.year == cursor.year && event.date.month == cursor.month)
        .map(|event| event.date)
        .collect();

    for day in 1..=month_days {
        let index = first_weekday + usize::from(day - 1);
        let column = (index % 7) as i32;
        let row = (index / 7) as i32;
        let left = GRID_LEFT + column * CELL_WIDTH;
        let top = GRID_TOP + row * CELL_HEIGHT;
        let cell_date = CalendarDate {
            year: cursor.year,
            month: cursor.month,
            day,
        };
        let is_selected = cell_date == cursor;
        let is_today = today.is_some_and(|value| value == cell_date);
        let has_event = days_with_events.contains(&cell_date);
        let border = PrimitiveStyleBuilder::new()
            .stroke_color(BinaryColor::On)
            .stroke_width(if is_selected { 3 } else { 1 })
            .fill_color(if is_selected {
                BinaryColor::On
            } else {
                BinaryColor::Off
            })
            .build();
        let ink = if is_selected {
            BinaryColor::Off
        } else {
            BinaryColor::On
        };

        Rectangle::new(Point::new(left, top), Size::new(54, 40))
            .into_styled(border)
            .draw(display)?;
        Text::new(
            &format!("{day:>2}"),
            Point::new(left + 14, top + 28),
            state.display.text_style(UiTextRole::Body, ink),
        )
        .draw(display)?;
        if is_today {
            Rectangle::new(Point::new(left + 43, top + 6), Size::new(5, 5))
                .into_styled(PrimitiveStyle::with_fill(ink))
                .draw(display)?;
        }
        if has_event {
            Rectangle::new(Point::new(left + 6, top + 31), Size::new(10, 3))
                .into_styled(PrimitiveStyle::with_fill(ink))
                .draw(display)?;
        }
    }
    Ok(())
}

fn draw_agenda_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    event: &CalendarEvent,
    selected: bool,
) -> Result<(), Infallible> {
    let box_style = if selected {
        PrimitiveStyle::with_fill(BinaryColor::On)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    let ink = if selected {
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    Rectangle::new(Point::new(22, top), Size::new(436, AGENDA_ROW_HEIGHT))
        .into_styled(box_style)
        .draw(display)?;
    Text::new(
        event.kind.label(),
        Point::new(36, top + 22),
        state.display.text_style(UiTextRole::Detail, ink),
    )
    .draw(display)?;
    Text::new(
        &compact_text(&event.title, 31),
        Point::new(112, top + 32),
        state.display.text_style(UiTextRole::Body, ink),
    )
    .draw(display)?;
    Ok(())
}

fn agenda_visible_range_label(
    locale: Locale,
    event_count: usize,
    visible: &core::ops::Range<usize>,
) -> String {
    if event_count == 0 {
        t(locale, "Showing 0 of 0", "Mostra 0 di 0").into()
    } else {
        match locale {
            Locale::English => format!(
                "Showing {}-{} of {event_count}",
                visible.start + 1,
                visible.end
            ),
            Locale::Italian => format!(
                "Mostra {}-{} di {event_count}",
                visible.start + 1,
                visible.end
            ),
        }
    }
}

fn detail_line(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    label: &str,
    value: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(label, Point::new(22, y), style).draw(display)?;
    Text::new(&compact_text(value, 37), Point::new(154, y), style).draw(display)?;
    Ok(())
}

fn draw_wrapped_detail(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    detail: &str,
    left: i32,
    first_baseline: i32,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let words = detail.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        Text::new(
            t(
                locale,
                "No additional detail.",
                "Nessun dettaglio aggiuntivo.",
            ),
            Point::new(left, first_baseline),
            state.display.body_style(),
        )
        .draw(display)?;
        return Ok(());
    }
    let mut line = String::new();
    let mut lines = Vec::new();
    for word in words {
        let proposed = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if proposed.chars().count() > 36 && !line.is_empty() {
            lines.push(line);
            line = word.to_string();
        } else {
            line = proposed;
        }
        if lines.len() == 3 {
            break;
        }
    }
    if !line.is_empty() && lines.len() < 3 {
        lines.push(line);
    }
    for (index, line) in lines.iter().enumerate() {
        Text::new(
            &compact_text(line, 36),
            Point::new(left, first_baseline + index as i32 * 34),
            state.display.body_style(),
        )
        .draw(display)?;
    }
    Ok(())
}

fn draw_action_row(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    label: &str,
    selected: bool,
) -> Result<(), Infallible> {
    let style = if selected {
        PrimitiveStyle::with_fill(BinaryColor::On)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    let ink = if selected {
        BinaryColor::Off
    } else {
        BinaryColor::On
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 42))
        .into_styled(style)
        .draw(display)?;
    Text::new(
        label,
        Point::new(40, top + 29),
        state.display.text_style(UiTextRole::Body, ink),
    )
    .draw(display)?;
    Ok(())
}

fn draw_editor_keyboard(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let editor = state
        .calendar
        .editor
        .as_ref()
        .expect("editor checked before keyboard");
    for (row_index, row) in CALENDAR_EDITOR_KEY_ROWS.iter().enumerate() {
        for (column_index, label) in row.iter().enumerate() {
            let index = row_index * 7 + column_index;
            let left = 22 + column_index as i32 * 62;
            let top = 344 + row_index as i32 * 54;
            let selected = editor.selected_key_index() == index;
            Rectangle::new(Point::new(left, top), Size::new(58, 46))
                .into_styled(PrimitiveStyle::with_stroke(
                    BinaryColor::On,
                    if selected { 3 } else { 1 },
                ))
                .draw(display)?;
            Text::new(
                label,
                Point::new(left + if label.len() > 3 { 4 } else { 15 }, top + 29),
                state.display.detail_style(),
            )
            .draw(display)?;
        }
    }
    Ok(())
}

fn truncate_or_placeholder(value: &str, placeholder: &str, max_chars: usize) -> String {
    if value.is_empty() {
        placeholder.into()
    } else {
        compact_text(value, max_chars)
    }
}

fn month_label(locale: Locale, month: u8) -> &'static str {
    let labels: &[&str; 12] = match locale {
        Locale::English => &MONTH_LABELS_EN,
        Locale::Italian => &MONTH_LABELS_IT,
    };
    month
        .checked_sub(1)
        .and_then(|index| labels.get(usize::from(index)))
        .copied()
        .unwrap_or(t(locale, "Unknown", "Sconosciuto"))
}

fn selected_date_label(locale: Locale, date: CalendarDate) -> String {
    const WEEKDAYS_EN: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const WEEKDAYS_IT: [&str; 7] = [
        "domenica",
        "lunedì",
        "martedì",
        "mercoledì",
        "giovedì",
        "venerdì",
        "sabato",
    ];
    let weekdays: &[&str; 7] = match locale {
        Locale::English => &WEEKDAYS_EN,
        Locale::Italian => &WEEKDAYS_IT,
    };
    let weekday = weekdays
        .get(usize::from(date.weekday()))
        .copied()
        .unwrap_or(t(locale, "Unknown", "Sconosciuto"));
    match locale {
        Locale::English => format!(
            "{weekday}, {} {}, {}",
            month_label(locale, date.month),
            date.day,
            date.year
        ),
        Locale::Italian => format!(
            "{weekday} {} {} {}",
            date.day,
            month_label(locale, date.month),
            date.year
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agenda_visible_range_label, month_label, selected_date_label, AGENDA_FIRST_ROW_TOP,
        AGENDA_FOOTER_HINT_EN, AGENDA_RANGE_BASELINE, AGENDA_ROW_HEIGHT, AGENDA_ROW_STEP,
        CALENDAR_EDITOR_FOOTER_HINT_EN,
    };
    use crate::{
        app::AppState,
        calendar::{CalendarDate, CALENDAR_AGENDA_VISIBLE_ROWS},
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        regional::Locale,
    };

    #[test]
    fn renders_readable_selected_date() {
        let date = CalendarDate::new(2026, 6, 4).unwrap();
        assert_eq!(month_label(Locale::English, 6), "June");
        assert_eq!(
            selected_date_label(Locale::English, date),
            "Thursday, June 4, 2026"
        );
        assert_eq!(month_label(Locale::Italian, 6), "giugno");
        assert_eq!(
            selected_date_label(Locale::Italian, date),
            "giovedì 4 giugno 2026"
        );
    }

    #[test]
    fn editor_and_delete_confirmation_render_without_sd_card() {
        let mut state = AppState::default();
        state.calendar.begin_create_personal();
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        super::render_calendar_event_editor(&mut display, &state).unwrap();
        super::render_calendar_delete_confirmation(&mut display, &state).unwrap();
    }

    #[test]
    fn editor_footer_fits_the_e_paper_width() {
        assert_eq!(CALENDAR_EDITOR_FOOTER_HINT_EN, "HOLD H/V  SELECT KEY");
        assert!(CALENDAR_EDITOR_FOOTER_HINT_EN.chars().count() <= 40);
    }

    #[test]
    fn agenda_range_labels_use_safe_vertical_bounds() {
        assert_eq!(
            agenda_visible_range_label(Locale::English, 0, &(0..0)),
            "Showing 0 of 0"
        );
        assert_eq!(
            agenda_visible_range_label(Locale::English, 1, &(0..1)),
            "Showing 1-1 of 1"
        );
        assert_eq!(
            agenda_visible_range_label(Locale::Italian, 1, &(0..1)),
            "Mostra 1-1 di 1"
        );
        assert!(AGENDA_RANGE_BASELINE < AGENDA_FIRST_ROW_TOP);
        let last_row_bottom = AGENDA_FIRST_ROW_TOP
            + (CALENDAR_AGENDA_VISIBLE_ROWS as i32 - 1) * AGENDA_ROW_STEP
            + AGENDA_ROW_HEIGHT as i32;
        assert!(last_row_bottom < 746);
    }

    #[test]
    fn agenda_footer_hint_is_compact_for_the_e_paper_width() {
        assert_eq!(AGENDA_FOOTER_HINT_EN, "HOLD ADD");
        assert!(AGENDA_FOOTER_HINT_EN.chars().count() <= 40);
    }
}
