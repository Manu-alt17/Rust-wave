//! Open-Meteo current conditions overview and readable cache details.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
    weather::DailyForecast,
};

pub fn render_weather(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
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
    let condition = current.map_or_else(
        || t(locale, "Weather unavailable", "Meteo non disponibile"),
        |value| value.condition_label_i18n(locale),
    );

    draw_header(display, state, t(locale, "WEATHER", "METEO"))?;

    Text::new(&weather.location, Point::new(22, 108), heading).draw(display)?;
    Text::new(condition, Point::new(22, 138), body).draw(display)?;
    line(
        display,
        180,
        t(locale, "Feels like", "Percepita"),
        &apparent,
        body,
    )?;
    line(
        display,
        212,
        t(locale, "Humidity", "Umidità"),
        &humidity,
        body,
    )?;
    line(display, 244, t(locale, "Wind", "Vento"), &wind, body)?;

    Text::new(
        t(locale, "Four-day forecast", "Previsioni a 4 giorni"),
        Point::new(22, 294),
        heading,
    )
    .draw(display)?;
    if weather.forecast.is_empty() {
        Text::new(
            if weather.state == crate::weather::WeatherFetchState::Retrying {
                t(
                    locale,
                    "No cached forecast. Retrying automatically.",
                    "Nessuna previsione salvata. Nuovo tentativo automatico.",
                )
            } else {
                t(
                    locale,
                    "No cached forecast. Choose Refresh.",
                    "Nessuna previsione salvata. Selezionare Aggiorna.",
                )
            },
            Point::new(22, 336),
            body,
        )
        .draw(display)?;
    } else {
        for (index, row) in weather.forecast.iter().take(4).enumerate() {
            draw_forecast_row(display, 328 + index as i32 * 52, row, locale, body)?;
        }
    }

    draw_action(
        display,
        558,
        t(locale, "Refresh weather", "Aggiorna meteo"),
        state.weather_action_selected == 0,
        body,
    )?;
    draw_action(
        display,
        612,
        t(locale, "Weather details", "Dettagli meteo"),
        state.weather_action_selected == 1,
        body,
    )?;
    draw_footer(display, state, t(locale, "SELECT RUN", "SELECT AVVIA"))?;
    Ok(())
}

pub fn render_weather_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let weather = &state.weather;
    let observed = weather
        .current
        .as_ref()
        .map_or(t(locale, "not fetched", "mai recuperato"), |value| {
            value.observed_at.as_str()
        });
    let error = weather
        .error
        .as_deref()
        .unwrap_or(t(locale, "none", "nessuno"));

    draw_header(
        display,
        state,
        t(locale, "WEATHER INFO", "INFO METEO"),
    )?;

    Text::new(
        t(locale, "Cached forecast", "Previsioni memorizzate"),
        Point::new(22, 114),
        heading,
    )
    .draw(display)?;
    line(
        display,
        158,
        t(locale, "Provider", "Fornitore"),
        &weather.provider,
        body,
    )?;
    line(
        display,
        192,
        t(locale, "Timezone", "Fuso orario"),
        &weather.provider_timezone,
        body,
    )?;
    line(
        display,
        226,
        t(locale, "Observed", "Rilevato"),
        observed,
        body,
    )?;
    line(
        display,
        260,
        t(locale, "Last success", "Ultimo successo"),
        weather.last_success_label_i18n(locale),
        body,
    )?;

    Text::new(
        t(locale, "Configuration", "Configurazione"),
        Point::new(22, 328),
        heading,
    )
    .draw(display)?;
    Text::new(
        crate::weather::WeatherSnapshot::config_path(),
        Point::new(22, 370),
        body,
    )
    .draw(display)?;

    Text::new(
        t(locale, "Last error", "Ultimo errore"),
        Point::new(22, 442),
        heading,
    )
    .draw(display)?;
    Text::new(error, Point::new(22, 484), detail).draw(display)?;
    Text::new(
        t(
            locale,
            "Press BOOT to return to Weather.",
            "Premere BOOT per tornare a Meteo.",
        ),
        Point::new(22, 620),
        body,
    )
    .draw(display)?;
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
    locale: Locale,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let precipitation = row
        .precipitation_probability_percent
        .map_or_else(|| "--%".into(), |value| format!("{value}%"));
    Text::new(
        &format!("{}  {}", row.date, row.condition_label_i18n(locale)),
        Point::new(22, y),
        style,
    )
    .draw(display)?;
    let stats = match locale {
        Locale::English => format!(
            "High {}F   Low {}F   POP {precipitation}",
            format_tenths(row.high_tenths_f),
            format_tenths(row.low_tenths_f)
        ),
        Locale::Italian => format!(
            "Massima {}F   Minima {}F   Prob {precipitation}",
            format_tenths(row.high_tenths_f),
            format_tenths(row.low_tenths_f)
        ),
    };
    Text::new(&stats, Point::new(22, y + 24), style).draw(display)?;
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
