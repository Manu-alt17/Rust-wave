//! Open-Meteo current conditions overview and readable cache details.

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
    weather::DailyForecast,
};

pub fn render_weather(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let weather = &state.weather;
    let current = weather.current.as_ref();
    let apparent = current.map_or_else(
        || "--.- F".into(),
        |value| value.apparent_temperature_label(),
    );
    let humidity = current.map_or_else(
        || "--%".into(),
        |value| format!("{}%", value.humidity_percent),
    );
    let wind = current.map_or_else(|| "--.- mph".into(), |value| value.wind_label());
    let condition = current.map_or("Weather unavailable", |value| value.condition_label());

    draw_header(display, state, "WEATHER")?;

    Text::new(&weather.location, Point::new(22, 108), heading).draw(display)?;
    Text::new(condition, Point::new(22, 138), body).draw(display)?;
    line(display, 180, "Feels like", &apparent, body)?;
    line(display, 212, "Humidity", &humidity, body)?;
    line(display, 244, "Wind", &wind, body)?;

    Text::new("Four-day forecast", Point::new(22, 294), heading).draw(display)?;
    if weather.forecast.is_empty() {
        Text::new(
            if weather.state == crate::weather::WeatherFetchState::Retrying {
                "No cached forecast. Retrying automatically."
            } else {
                "No cached forecast. Choose Refresh."
            },
            Point::new(22, 336),
            body,
        )
        .draw(display)?;
    } else {
        for (index, row) in weather.forecast.iter().take(4).enumerate() {
            draw_forecast_row(display, 328 + index as i32 * 52, row, body)?;
        }
    }

    draw_action(
        display,
        558,
        "Refresh weather",
        state.weather_action_selected == 0,
        body,
    )?;
    draw_action(
        display,
        612,
        "Weather details",
        state.weather_action_selected == 1,
        body,
    )?;
    draw_footer(display, state.display, "UP/DOWN  SELECT RUN  BOOT BACK")?;
    Ok(())
}

pub fn render_weather_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let weather = &state.weather;
    let observed = weather
        .current
        .as_ref()
        .map_or("not fetched", |value| value.observed_at.as_str());
    let error = weather.error.as_deref().unwrap_or("none");

    draw_header(display, state, "WEATHER DETAILS")?;

    Text::new("Cached forecast", Point::new(22, 114), heading).draw(display)?;
    line(display, 158, "Provider", &weather.provider, body)?;
    line(display, 192, "Timezone", &weather.provider_timezone, body)?;
    line(display, 226, "Observed", observed, body)?;
    line(
        display,
        260,
        "Last success",
        weather.last_success_label(),
        body,
    )?;

    Text::new("Configuration", Point::new(22, 328), heading).draw(display)?;
    Text::new(
        crate::weather::WeatherSnapshot::config_path(),
        Point::new(22, 370),
        body,
    )
    .draw(display)?;

    Text::new("Last error", Point::new(22, 442), heading).draw(display)?;
    Text::new(error, Point::new(22, 484), detail).draw(display)?;
    Text::new(
        "Press BOOT to return to Weather.",
        Point::new(22, 620),
        body,
    )
    .draw(display)?;
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
    Text::new(value, Point::new(166, y), style).draw(display)?;
    Ok(())
}

fn draw_forecast_row(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    row: &DailyForecast,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let precipitation = row
        .precipitation_probability_percent
        .map_or_else(|| "--%".into(), |value| format!("{value}%"));
    Text::new(
        &format!("{}  {}", row.date, row.condition_label()),
        Point::new(22, y),
        style,
    )
    .draw(display)?;
    Text::new(
        &format!(
            "High {}F   Low {}F   POP {precipitation}",
            format_tenths(row.high_tenths_f),
            format_tenths(row.low_tenths_f)
        ),
        Point::new(22, y + 24),
        style,
    )
    .draw(display)?;
    Ok(())
}

fn format_tenths(value: i16) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let magnitude = i32::from(value).abs();
    format!("{sign}{}.{:01}", magnitude / 10, magnitude % 10)
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

#[cfg(test)]
mod tests {
    use super::{render_weather, render_weather_details};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn weather_overview_and_details_render_without_cache() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_weather(&mut display, &state).unwrap();
        render_weather_details(&mut display, &state).unwrap();
    }
}
