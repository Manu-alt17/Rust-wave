//! SD-backed RTC alarm scheduling and active-alarm actions.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    alarm::ALARMS_CONFIG_PATH,
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

pub fn render_alarms(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let alarms = &state.alarms;
    let next = alarms.next_label_i18n(locale);

    draw_header(display, state, t(locale, "ALARMS", "SVEGLIE"))?;

    if let Some(active) = alarms.active.as_ref() {
        Text::new(
            t(locale, "Alarm active", "Sveglia attiva"),
            Point::new(22, 112),
            heading,
        )
        .draw(display)?;
        Text::new(&active.label_i18n(locale), Point::new(22, 156), body).draw(display)?;
        Text::new(
            state.audio.alarm_label_i18n(locale),
            Point::new(22, 194),
            body,
        )
        .draw(display)?;
        draw_action(
            display,
            272,
            t(locale, "Snooze", "Posticipa"),
            alarms.selected == 0,
            body,
        )?;
        draw_action(
            display,
            340,
            t(locale, "Dismiss", "Interrompi"),
            alarms.selected == 1,
            body,
        )?;
        let snooze_hint = match locale {
            Locale::English => format!("Snooze interval: {} minutes", alarms.snooze_minutes),
            Locale::Italian => format!("Intervallo di posticipo: {} minuti", alarms.snooze_minutes),
        };
        Text::new(&snooze_hint, Point::new(22, 444), body).draw(display)?;
        draw_footer(display, state, t(locale, "SELECT RUN", "SELECT ESEGUI"))?;
        return Ok(());
    }

    if let Some(editor) = alarms.editor.as_ref() {
        Text::new(
            t(locale, "Runtime editor", "Editor runtime"),
            Point::new(22, 108),
            heading,
        )
        .draw(display)?;
        let alarm_name_hint = match locale {
            Locale::English => format!("Alarm: {}", editor.draft.name),
            Locale::Italian => format!("Sveglia: {}", editor.draft.name),
        };
        Text::new(&alarm_name_hint, Point::new(22, 148), body).draw(display)?;
        draw_editor_row(
            display,
            192,
            t(locale, "Hour", "Ora"),
            &format!("{:02}", editor.draft.hour),
            editor.field_index == 0,
            body,
        )?;
        draw_editor_row(
            display,
            248,
            t(locale, "Minute", "Minuto"),
            &format!("{:02}", editor.draft.minute),
            editor.field_index == 1,
            body,
        )?;
        draw_editor_row(
            display,
            304,
            t(locale, "Enabled", "Attivo"),
            editor.draft.status_label_i18n(locale),
            editor.field_index == 2,
            body,
        )?;
        draw_editor_row(
            display,
            360,
            t(locale, "Mode", "Modalità"),
            match editor.draft.schedule {
                crate::alarm::AlarmScheduleKind::Recurring { .. } => {
                    t(locale, "RECURRING", "RICORRENTE")
                }
                crate::alarm::AlarmScheduleKind::OneTime { .. } => {
                    t(locale, "ONE TIME", "UNA VOLTA")
                }
            },
            editor.field_index == 3,
            body,
        )?;
        draw_editor_row(
            display,
            416,
            t(locale, "Weekdays / date", "Giorni / data"),
            &editor.draft.schedule.compact_label(),
            editor.field_index == 4,
            body,
        )?;
        draw_action(
            display,
            502,
            t(locale, "Save runtime edit", "Salva modifica temporanea"),
            editor.field_index == 5,
            body,
        )?;
        Text::new(
            t(
                locale,
                "Edit ALARMS.TXT for persistent changes.",
                "Modifica ALARMS.TXT per rendere le modifiche permanenti.",
            ),
            Point::new(22, 586),
            body,
        )
        .draw(display)?;
        draw_footer(
            display,
            state,
            t(
                locale,
                "UP/DOWN CHANGE  SELECT NEXT",
                "SU/GIU CAMBIA  SELECT AVANTI",
            ),
        )?;
        return Ok(());
    }

    Text::new(
        t(locale, "Configured schedules", "Programmi configurati"),
        Point::new(22, 108),
        heading,
    )
    .draw(display)?;
    let next_hint = match locale {
        Locale::English => format!("Next: {next}"),
        Locale::Italian => format!("Prossima: {next}"),
    };
    Text::new(&next_hint, Point::new(22, 148), body).draw(display)?;
    let config_hint = match locale {
        Locale::English => format!("Config: {ALARMS_CONFIG_PATH}"),
        Locale::Italian => format!("Configurazione: {ALARMS_CONFIG_PATH}"),
    };
    Text::new(&config_hint, Point::new(22, 184), body).draw(display)?;

    if alarms.alarms.is_empty() {
        Text::new(
            t(
                locale,
                "No alarm schedules were loaded.",
                "Nessun programma di sveglia caricato.",
            ),
            Point::new(22, 252),
            body,
        )
        .draw(display)?;
        Text::new(
            t(
                locale,
                "Add alarm rows to ALARMS.TXT.",
                "Aggiungi righe sveglia in ALARMS.TXT.",
            ),
            Point::new(22, 292),
            body,
        )
        .draw(display)?;
    } else {
        for (index, alarm) in alarms.alarms.iter().take(6).enumerate() {
            draw_alarm_row(
                display,
                218 + index as i32 * 62,
                alarm,
                alarms.selected == index,
                body,
                locale,
            )?;
        }
    }

    if let Some(error) = alarms.error.as_deref() {
        let error_hint = match locale {
            Locale::English => format!("Last error: {error}"),
            Locale::Italian => format!("Ultimo errore: {error}"),
        };
        Text::new(&error_hint, Point::new(22, 628), body).draw(display)?;
    }
    draw_footer(display, state, t(locale, "SELECT EDIT", "SELECT MODIFICA"))?;
    Ok(())
}

fn draw_alarm_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    alarm: &crate::alarm::AlarmDefinition,
    selected: bool,
    style: UiTextStyle,
    locale: Locale,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(36, top + 34),
        style,
    )
    .draw(display)?;
    Text::new(&alarm.name, Point::new(58, top + 34), style).draw(display)?;
    Text::new(&alarm.time_label(), Point::new(206, top + 34), style).draw(display)?;
    Text::new(
        &alarm.schedule.compact_label_i18n(locale),
        Point::new(274, top + 34),
        style,
    )
    .draw(display)?;
    Text::new(
        alarm.status_label_i18n(locale),
        Point::new(420, top + 34),
        style,
    )
    .draw(display)?;
    Ok(())
}

fn draw_editor_row(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    value: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 46))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 30),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 30), style).draw(display)?;
    Text::new(value, Point::new(248, top + 30), style).draw(display)?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let border = if selected {
        PrimitiveStyle::with_stroke(BinaryColor::On, 4)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 34),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 34), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_alarms;
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn alarms_screen_renders_without_loaded_configuration() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_alarms(&mut display, &AppState::default()).unwrap();
    }
}
