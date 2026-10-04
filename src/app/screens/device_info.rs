//! Read-only firmware, board and runtime information split across readable pages.

use core::convert::Infallible;

use crate::{
    app::{
        i18n::t,
        state::{AppState, SettingsResetStage},
        widgets::{
            footer::{back_only, draw_footer, footer_hints, select_and_back, FooterKey},
            header::draw_header,
            layout::{CONTENT_BOTTOM, CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE},
            list::{draw_field, draw_list_row, draw_section_title, ROW_HEIGHT, ROW_STEP},
            text::draw_paragraph,
        },
    },
    build_info::{FIRMWARE_VERSION, PRODUCT_NAME},
    orientation::OrientedFrameBuffer,
    panel_refresh::PANEL_PARTIAL_REFRESH_LIMIT,
};

/// Top of the "next page" row, anchored to the bottom of the content area.
const NEXT_ROW_TOP: i32 = CONTENT_BOTTOM - ROW_HEIGHT;

/// Page 1/3: product firmware and display.
pub fn render_device_info(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let partials = format!(
        "{} / {PANEL_PARTIAL_REFRESH_LIMIT}",
        state.partial_refreshes
    );
    let unknown = t(locale, "unknown", "sconosciuto");

    draw_header(display, state, "INFO")?;

    let mut baseline = draw_section_title(display, preferences, FIRST_BASELINE, "Firmware")?;
    let firmware: [(&str, &str); 3] = [
        (t(locale, "Product", "Prodotto"), PRODUCT_NAME),
        (t(locale, "Version", "Versione"), FIRMWARE_VERSION),
        (
            "Bootloader",
            state.installed_bootloader.as_deref().unwrap_or(unknown),
        ),
    ];
    for (label, value) in firmware {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Screen", "Schermo"),
    )?;
    let screen: [(&str, &str); 2] = [
        (
            t(locale, "Resolution", "Risoluzione"),
            t(
                locale,
                "480 x 800, black and white",
                "480 x 800, bianco e nero",
            ),
        ),
        (
            t(
                locale,
                "Refreshes since last cleaning",
                "Aggiornamenti dall'ultima pulizia",
            ),
            &partials,
        ),
    ];
    for (label, value) in screen {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    // "Restore settings" armed: what it touches is said before the second
    // SELECT.
    if state.settings_reset == SettingsResetStage::Armed {
        draw_paragraph(
            display,
            t(
                locale,
                "Text size, standby, sleep screen, most used settings and update channel go back to their first values. Language, clock, Wi-Fi and books stay.",
                "Dimensione del testo, standby, schermata di riposo, voci pi\u{00F9} usate e canale aggiornamenti tornano ai valori iniziali. Lingua, orologio, Wi-Fi e libri restano.",
            ),
            CONTENT_LEFT,
            baseline + 14,
            preferences.detail_style(),
            CONTENT_WIDTH,
            5,
            4,
        )?;
    }

    draw_list_row(
        display,
        preferences,
        NEXT_ROW_TOP - ROW_STEP,
        t(locale, "Memory card", "Scheda di memoria"),
        "",
        state.info_selected == 0,
    )?;
    let (reset_label, reset_value) = match state.settings_reset {
        SettingsResetStage::Idle => (t(locale, "Restore settings", "Ripristina impostazioni"), ""),
        SettingsResetStage::Armed => (t(locale, "Confirm: restore", "Conferma: ripristina"), ""),
        SettingsResetStage::Done => (
            t(locale, "Restore settings", "Ripristina impostazioni"),
            t(locale, "done", "fatto"),
        ),
    };
    draw_list_row(
        display,
        preferences,
        NEXT_ROW_TOP,
        reset_label,
        reset_value,
        state.info_selected == 1,
    )?;
    let hint = if state.settings_reset == SettingsResetStage::Armed {
        footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "CONFIRM", "CONFERMA")),
                (FooterKey::Boot, t(locale, "CANCEL", "ANNULLA")),
            ],
        )
    } else if state.info_selected == 1 {
        select_and_back(locale, t(locale, "RESTORE", "RIPRISTINA"))
    } else {
        select_and_back(locale, t(locale, "NEXT", "AVANTI"))
    };
    draw_footer(display, state, &hint)
}

/// Page 2/3: the memory card.
pub fn render_device_info_board(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let card = if !state.storage.mounted {
        t(locale, "Not found", "Non trovata")
    } else if state.storage.error.is_some() {
        t(locale, "Read error", "Errore di lettura")
    } else {
        t(locale, "Ready", "Pronta")
    };

    draw_header(display, state, "INFO")?;

    let mut baseline = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Memory card", "Scheda di memoria"),
    )?;
    let fields: [(&str, &str); 3] = [
        (t(locale, "Card", "Scheda"), card),
        (t(locale, "Format", "Formato"), "FAT, SDMMC 4 bit"),
        (
            t(locale, "Pins", "Pin"),
            "CLK16 CMD17 D0=15 D1=7 D2=8 D3=18",
        ),
    ];
    for (label, value) in fields {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    draw_list_row(
        display,
        preferences,
        NEXT_ROW_TOP,
        t(locale, "Network and keys", "Rete e tasti"),
        "",
        true,
    )?;
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "NEXT", "AVANTI")),
    )
}

/// Page 3/3: network status and the keys.
pub fn render_device_info_runtime(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let timezone = state.regional.timezone_label_for_rtc(state.board.rtc);

    draw_header(display, state, "INFO")?;

    let mut baseline = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Network", "Rete"),
    )?;
    let network: [(&str, &str); 3] = [
        ("Wi-Fi", state.network.wifi_state.status_text(locale)),
        (
            t(locale, "IP address", "Indirizzo IP"),
            state.network.ipv4_label(),
        ),
        (t(locale, "Time zone", "Fuso orario"), &timezone),
    ];
    for (label, value) in network {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Keys", "Tasti"),
    )?;
    let keys: [(&str, &str); 3] = [
        (
            t(locale, "Rocker", "Rotella"),
            t(
                locale,
                "Up, down, press: SELECT",
                "Su, gi\u{00F9}, pressione: SELECT",
            ),
        ),
        ("BOOT", t(locale, "Back", "Indietro")),
        (
            "Power",
            t(
                locale,
                "Short press: standby. Long press: screen menu",
                "Pressione breve: standby. Lunga: menu schermo",
            ),
        ),
    ];
    for (label, value) in keys {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }
    draw_footer(display, state, &back_only(locale))
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
