//! RTC-backed Clock overview, readable details page and the runtime "Set
//! date & time" editor.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
};

/// Draw the user-facing RTC overview.
pub fn render_clock(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let large = state.display.large_style();
    let outline = PrimitiveStyle::with_stroke(BinaryColor::On, 2);
    let time = state.board.time_label(state.regional);
    let date_time = state.board.date_time_label(state.regional);
    let battery = state.board.battery_label();
    let temperature = state
        .board
        .temperature_label(state.regional.temperature_unit);
    let humidity = state.board.humidity_label();

    draw_header(display, state, "CLOCK")?;

    Rectangle::new(Point::new(22, 100), Size::new(436, 150))
        .into_styled(outline)
        .draw(display)?;
    Text::new("Current RTC time", Point::new(42, 134), heading).draw(display)?;
    Text::new(&time, Point::new(42, 192), large).draw(display)?;
    Text::new(&date_time, Point::new(42, 228), body).draw(display)?;

    Text::new("Onboard status", Point::new(22, 310), heading).draw(display)?;
    line(display, 358, "Temperature", &temperature, body)?;
    line(display, 396, "Humidity", &humidity, body)?;
    line(display, 434, "Battery", &battery, body)?;

    if let Some(power) = state.board.power {
        let usb = if power.vbus_present {
            "Connected"
        } else {
            "Not detected"
        };
        let charge = if power.charging {
            "Charging"
        } else {
            "Not charging"
        };
        line(display, 472, "USB", usb, body)?;
        line(display, 510, "Charge state", charge, body)?;
    }

    draw_action(
        display,
        558,
        "Set date & time",
        state.clock_action_selected == 0,
        body,
    )?;
    draw_action(
        display,
        612,
        "RTC details",
        state.clock_action_selected == 1,
        body,
    )?;
    draw_footer(display, state.display, "UP/DOWN  SELECT OPEN  BOOT BACK")?;
    Ok(())
}

/// Draw the runtime-only local wall-clock editor opened from the Clock
/// overview's "Set date & time" action.
pub fn render_clock_set_time(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();

    draw_header(display, state, "SET DATE & TIME")?;

    let Some(editor) = state.clock_time_editor.as_ref() else {
        Text::new("No active edit.", Point::new(22, 140), body).draw(display)?;
        draw_footer(display, state.display, "BOOT BACK")?;
        return Ok(());
    };

    Text::new("Local wall clock", Point::new(22, 108), heading).draw(display)?;
    Text::new(
        &format!(
            "{}  {}",
            editor.draft.date_label(),
            editor.draft.time_label()
        ),
        Point::new(22, 148),
        body,
    )
    .draw(display)?;

    draw_editor_row(
        display,
        192,
        "Timezone",
        editor.timezone.name(),
        editor.field_index == 0,
        body,
    )?;
    draw_editor_row(
        display,
        248,
        "Hour",
        &format!("{:02}", editor.draft.hour),
        editor.field_index == 1,
        body,
    )?;
    draw_editor_row(
        display,
        304,
        "Minute",
        &format!("{:02}", editor.draft.minute),
        editor.field_index == 2,
        body,
    )?;
    draw_editor_row(
        display,
        360,
        "Year",
        &format!("{:04}", editor.draft.year),
        editor.field_index == 3,
        body,
    )?;
    draw_editor_row(
        display,
        416,
        "Month",
        &format!("{:02}", editor.draft.month),
        editor.field_index == 4,
        body,
    )?;
    draw_editor_row(
        display,
        472,
        "Day",
        &format!("{:02}", editor.draft.day),
        editor.field_index == 5,
        body,
    )?;
    draw_action(
        display,
        528,
        "Save date & time",
        editor.field_index == 6,
        body,
    )?;

    Text::new(
        "Changes apply to the on-board RTC immediately.",
        Point::new(22, 592),
        body,
    )
    .draw(display)?;
    draw_footer(
        display,
        state.display,
        "UP/DOWN CHANGE  SELECT NEXT  BOOT BACK",
    )?;
    Ok(())
}

/// Draw timezone, storage-basis and power details without crowding the overview.
pub fn render_clock_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let timezone = state.regional.timezone_label_for_rtc(state.board.rtc);
    let rtc_storage = state.regional.rtc_storage_label();
    let rtc_health = if state.board.rtc_clock_integrity_was_lost {
        "Cleared during startup"
    } else {
        "Clear"
    };
    let battery_voltage = state.board.power.map_or_else(
        || "Unavailable".into(),
        |power| {
            power
                .battery_voltage_mv
                .map_or_else(|| "Unavailable".into(), |mv| format!("{mv} mV"))
        },
    );

    draw_header(display, state, "RTC DETAILS")?;

    Text::new("Time basis", Point::new(22, 118), heading).draw(display)?;
    line(display, 164, "Display zone", &timezone, body)?;
    line(display, 204, "RTC storage", &rtc_storage, body)?;
    line(display, 244, "Integrity", rtc_health, body)?;

    Text::new("Power", Point::new(22, 318), heading).draw(display)?;
    line(display, 364, "Battery voltage", &battery_voltage, body)?;
    if let Some(power) = state.board.power {
        line(
            display,
            404,
            "USB VBUS",
            if power.vbus_present {
                "Connected"
            } else {
                "Not detected"
            },
            body,
        )?;
        line(
            display,
            444,
            "Charge state",
            if power.charging {
                "Charging"
            } else {
                "Not charging"
            },
            body,
        )?;
    }

    Text::new("Refresh policy", Point::new(22, 522), heading).draw(display)?;
    line(display, 568, "Live refresh", "30 seconds", body)?;
    line(display, 608, "Idle sleep", "60 seconds", body)?;
    draw_footer(display, state.display, "BOOT BACK")?;
    Ok(())
}

fn line(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    label: &str,
    value: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(label, Point::new(22, y), style).draw(display)?;
    Text::new(value, Point::new(196, y), style).draw(display)?;
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
    Rectangle::new(Point::new(22, top), Size::new(436, 44))
        .into_styled(border)
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 29),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 29), style).draw(display)?;
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
