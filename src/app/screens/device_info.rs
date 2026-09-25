//! Read-only firmware, board and runtime information split across readable pages.

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
    build_info::{FIRMWARE_VERSION, PRODUCT_NAME},
    orientation::OrientedFrameBuffer,
    panel_refresh::PANEL_PARTIAL_REFRESH_LIMIT,
};

/// Page 1/3: product firmware and display contract.
pub fn render_device_info(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let partials = format!("{} / {PANEL_PARTIAL_REFRESH_LIMIT}", state.partial_refreshes);

    // "Info" reads the same in English and Italian chrome, so this header
    // does not need a locale-branched `t()` call.
    draw_header(display, state, "INFO")?;

    Text::new(
        t(locale, "Firmware", "Firmware"),
        Point::new(22, 118),
        heading,
    )
    .draw(display)?;
    line(
        display,
        166,
        t(locale, "Product", "Prodotto"),
        PRODUCT_NAME,
        body,
    )?;
    line(
        display,
        206,
        t(locale, "Version", "Versione"),
        FIRMWARE_VERSION,
        body,
    )?;
    line(
        display,
        246,
        t(locale, "Milestone", "Milestone"),
        t(locale, "Readability repair", "Correzione leggibilità"),
        body,
    )?;

    Text::new(
        t(locale, "Display", "Display"),
        Point::new(22, 326),
        heading,
    )
    .draw(display)?;
    line(
        display,
        374,
        t(locale, "Logical UI", "UI logica"),
        t(locale, "480 x 800 portrait", "480 x 800 verticale"),
        body,
    )?;
    line(
        display,
        414,
        t(locale, "Native panel", "Pannello nativo"),
        t(locale, "800 x 480 mono", "800 x 480 mono"),
        body,
    )?;
    line(
        display,
        454,
        t(locale, "Framebuffer", "Framebuffer"),
        t(locale, "48,000 bytes / 1-bpp", "48.000 byte / 1-bpp"),
        body,
    )?;
    line(
        display,
        494,
        t(locale, "Partial chain", "Catena parziale"),
        &partials,
        body,
    )?;

    draw_action(
        display,
        594,
        t(locale, "Board services", "Servizi scheda"),
        body,
    )?;
    draw_footer(display, state, t(locale, "SELECT NEXT", "SELECT AVANTI"))?;
    Ok(())
}

/// Page 2/3: onboard services and read-only storage contract.
pub fn render_device_info_board(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    // "Info" reads the same in English and Italian chrome, so this header
    // does not need a locale-branched `t()` call.
    draw_header(display, state, "INFO")?;

    Text::new(
        t(locale, "SDMMC storage", "Archiviazione SDMMC"),
        Point::new(22, 118),
        heading,
    )
    .draw(display)?;
    line(
        display,
        166,
        t(locale, "Mount", "Montaggio"),
        state.storage.status_label(),
        body,
    )?;
    line(
        display,
        206,
        t(locale, "Mode", "Modalità"),
        t(
            locale,
            "4-bit FAT / read-only UI",
            "FAT a 4 bit / UI sola lettura",
        ),
        body,
    )?;
    Text::new(t(locale, "Pins", "Pin"), Point::new(22, 246), body).draw(display)?;
    Text::new(
        "CLK16 CMD17 D0=15 D1=7 D2=8 D3=18",
        Point::new(22, 280),
        detail,
    )
    .draw(display)?;

    draw_action(
        display,
        348,
        t(locale, "Runtime services", "Servizi runtime"),
        body,
    )?;
    draw_footer(display, state, t(locale, "SELECT NEXT", "SELECT AVANTI"))?;
    Ok(())
}

/// Page 3/3: network status and stable hardware ownership.
pub fn render_device_info_runtime(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let timezone = state.regional.timezone_label_for_rtc(state.board.rtc);

    // "Info" reads the same in English and Italian chrome, so this header
    // does not need a locale-branched `t()` call.
    draw_header(display, state, "INFO")?;

    Text::new(
        t(locale, "Runtime services", "Servizi runtime"),
        Point::new(22, 118),
        heading,
    )
    .draw(display)?;
    line(
        display,
        166,
        t(locale, "Network", "Rete"),
        state.network.home_badge(),
        body,
    )?;
    line(
        display,
        206,
        t(locale, "Weather", "Meteo"),
        state.weather.home_badge(),
        body,
    )?;
    line(
        display,
        246,
        t(locale, "RTC alarms", "Sveglie RTC"),
        state.alarms.home_badge(),
        body,
    )?;
    line(
        display,
        286,
        t(locale, "Display zone", "Fuso orario"),
        &timezone,
        body,
    )?;
    line(
        display,
        326,
        t(locale, "Temperature", "Temperatura"),
        state.regional.temperature_unit.marker(),
        body,
    )?;

    Text::new(
        t(locale, "Stable ownership", "Risorse hardware stabili"),
        Point::new(22, 404),
        heading,
    )
    .draw(display)?;
    line(
        display,
        452,
        t(locale, "EPD busy", "EPD busy"),
        t(locale, "GPIO3 / ALDO3 managed", "GPIO3 / gestito da ALDO3"),
        body,
    )?;
    line(
        display,
        492,
        t(locale, "Buttons", "Pulsanti"),
        "UP4 SELECT5 DOWN6",
        body,
    )?;
    line(
        display,
        532,
        t(locale, "Power key", "Tasto accensione"),
        t(
            locale,
            "Hold menu / short sleep",
            "Pressione lunga menu / breve sospensione",
        ),
        body,
    )?;
    line(
        display,
        572,
        t(locale, "RTC alarm", "Sveglia RTC"),
        t(locale, "GPIO45 active-low", "GPIO45 attivo basso"),
        body,
    )?;
    Text::new(
        t(
            locale,
            "Press BOOT to return to page 2.",
            "Premere BOOT per tornare alla pagina 2.",
        ),
        Point::new(22, 634),
        detail,
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
    Text::new(value, Point::new(194, y), style).draw(display)?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 4))
        .draw(display)?;
    Text::new(">", Point::new(38, top + 34), style).draw(display)?;
    Text::new(label, Point::new(68, top + 34), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_device_info, render_device_info_board, render_device_info_runtime};
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn device_info_pages_render_without_optional_services() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_device_info(&mut display, &state).unwrap();
        render_device_info_board(&mut display, &state).unwrap();
        render_device_info_runtime(&mut display, &state).unwrap();
    }
}
