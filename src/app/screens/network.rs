//! Wi-Fi, SNTP, phone Wi-Fi provisioning and explicitly activated SD-card
//! transfer portal screens.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{Circle, CornerRadii, PrimitiveStyle, Rectangle, RoundedRectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::Text,
        widgets::{
            footer::{
                back_action, back_only, draw_footer, draw_footer_paged, footer_hints,
                select_and_back, FooterKey,
            },
            header::draw_header,
            layout::{CONTENT_LEFT, CONTENT_RIGHT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{
                centered_baseline, draw_field, draw_list_row, draw_row_frame, draw_section_title,
                ROW_STEP,
            },
            qr::draw_qr,
            text::{draw_paragraph, draw_text_centered, draw_text_fit},
        },
    },
    network::NetworkSnapshot,
    orientation::OrientedFrameBuffer,
    regional::Locale,
    wifi_transfer::{JoinAttemptState, WifiTransferSnapshot, WifiTransferState},
};

pub fn render_network(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let network = &state.network;
    let rssi = network.rssi_label();

    draw_header(display, state, t(locale, "NETWORK", "RETE"))?;

    let mut baseline = draw_section_title(display, preferences, FIRST_BASELINE, "Wi-Fi")?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Status", "Stato"),
        network.wifi_state.status_text(locale),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Network", "Rete"),
        network.ssid_label(),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "IP address", "Indirizzo IP"),
        network.ipv4_label(),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Signal", "Segnale"),
        &rssi,
    )?;

    let body_line = i32::from(preferences.body_style().line_height());
    let mut rows_top = baseline - body_line + 10;
    if let Some(ssid) = state.network_join_failed.as_deref() {
        // A "Connect" that did not work: say which network, and that the
        // device went back to the saved ones.
        let notice = match locale {
            Locale::English => {
                format!("Could not connect to {ssid}. Trying the saved networks again.")
            }
            Locale::Italian => {
                format!("Connessione a {ssid} non riuscita. Riprovo con le reti salvate.")
            }
        };
        let next = draw_paragraph(
            display,
            &notice,
            CONTENT_LEFT,
            baseline,
            preferences.detail_style(),
            CONTENT_WIDTH,
            2,
            2,
        )?;
        rows_top = next - i32::from(preferences.detail_style().line_height()) + 8;
    }
    let saved = network.saved_network_count.to_string();
    let rows: [(&str, &str); 4] = [
        (
            t(locale, "Configure via phone", "Configura da telefono"),
            "",
        ),
        (t(locale, "Saved networks", "Reti salvate"), &saved),
        (t(locale, "Retry connection", "Riprova connessione"), ""),
        (t(locale, "Details", "Dettagli"), ""),
    ];
    for (index, (label, value)) in rows.into_iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            rows_top + index as i32 * ROW_STEP,
            label,
            value,
            state.network_action_selected == index,
        )?;
    }
    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "OPEN", "APRI")),
    )
}

/// Single-sentence phone Wi-Fi join status, matching the portal card's
/// "waiting / connecting / connected" copy instead of the terser
/// label-plus-SSID pair the old two-line layout used.
fn provision_join_status_text(join: &JoinAttemptState, locale: Locale) -> String {
    match join {
        JoinAttemptState::Idle => t(
            locale,
            "Waiting for the phone to connect...",
            "In attesa che il telefono si connetta...",
        )
        .to_string(),
        JoinAttemptState::Testing { ssid } => {
            format!("{} {ssid}...", t(locale, "Connecting to", "Connessione a"))
        }
        JoinAttemptState::Succeeded { ssid } => {
            format!("{}: {ssid}", t(locale, "Connected", "Connesso"))
        }
        JoinAttemptState::Failed { ssid, error } => {
            format!("{}: {ssid} ({error})", t(locale, "Failed", "Non riuscita"))
        }
    }
}

/// Saved-network list, or the action menu of the selected network once
/// SELECT opened it: connect to it, forget it. Adding a network or changing
/// its password happens through the phone portal.
pub fn render_network_saved(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let saved = &state.network_saved;

    draw_header(display, state, t(locale, "SAVED WI-FI", "RETI SALVATE"))?;

    if saved.networks.is_empty() {
        let next = draw_section_title(
            display,
            preferences,
            FIRST_BASELINE,
            t(locale, "No saved network", "Nessuna rete salvata"),
        )?;
        draw_paragraph(
            display,
            t(
                locale,
                "Choose Configure via phone on the Network screen to add one.",
                "Scegli Configura da telefono nella schermata Rete per aggiungerne una.",
            ),
            CONTENT_LEFT,
            next,
            preferences.body_style(),
            CONTENT_WIDTH,
            4,
            6,
        )?;
        return draw_footer(display, state, &back_only(locale));
    }

    if let Some(menu_selected) = saved.menu {
        let ssid = saved
            .selected_entry()
            .map_or("", |entry| entry.ssid.as_str());
        draw_section_title(display, preferences, FIRST_BASELINE, ssid)?;
        let rows_top = FIRST_BASELINE + 24;
        for (index, action) in saved.menu_actions().into_iter().enumerate() {
            draw_list_row(
                display,
                preferences,
                rows_top + index as i32 * ROW_STEP,
                action.label_i18n(locale),
                "",
                index == menu_selected,
            )?;
        }
        return draw_footer(
            display,
            state,
            &select_and_back(locale, t(locale, "CONFIRM", "CONFERMA")),
        );
    }

    let selected_on_page = saved.selected_on_page();
    for (index, entry) in saved.visible_entries().iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            FIRST_ROW_TOP + index as i32 * ROW_STEP,
            if entry.ssid.is_empty() {
                "-"
            } else {
                &entry.ssid
            },
            if entry.connected {
                t(locale, "Connected", "Connessa")
            } else {
                ""
            },
            index == selected_on_page,
        )?;
    }
    draw_footer_paged(
        display,
        state,
        &select_and_back(locale, t(locale, "OPTIONS", "OPZIONI")),
        Some(saved.page_position()),
    )
}

/// Inner padding of the status card holding the QR code and connection
/// details.
const PORTAL_CARD_PAD: i32 = 20;
/// Target side length of the QR module, vertically centered in the card;
/// the actual drawn size still self-sizes to a whole number of modules.
const PORTAL_QR_BOX: i32 = 140;
const PORTAL_CARD_HEIGHT: i32 = 216;
const PORTAL_CARD_CORNER_RADIUS: Size = Size::new(16, 16);
const PORTAL_DOT_DIAMETER: i32 = 12;
/// Height of the "stop" row under the status.
const PORTAL_BUTTON_HEIGHT: i32 = 64;

pub fn render_wifi_transfer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let heading = preferences.heading_style();
    let body = preferences.body_style();
    let detail = preferences.detail_style();
    let transfer = &state.wifi_transfer;
    // No Wi-Fi joined yet: the portal was bootstrapped from the device's own
    // hotspot (see `NetworkRuntime::start_provisioning`) instead of the
    // already-connected LAN, so the QR below joins that hotspot first.
    let via_hotspot = transfer.ap_ssid.is_some();
    let ready = transfer.state == WifiTransferState::Ready;

    draw_header(display, state, t(locale, "OVER WI-FI", "VIA WI-FI"))?;

    let text_left = CONTENT_LEFT + 18;
    let text_width = CONTENT_RIGHT - text_left;
    let status_baseline = 96;
    draw_status_dot(display, CONTENT_LEFT, status_baseline, ready)?;
    draw_text_fit(
        display,
        wifi_transfer_status_label(transfer.state, locale),
        Point::new(text_left, status_baseline),
        heading,
        text_width,
    )?;
    let hint_baseline = status_baseline + i32::from(heading.line_height());
    let after_hint = draw_paragraph(
        display,
        wifi_transfer_mode_hint(transfer.state, via_hotspot, locale),
        text_left,
        hint_baseline,
        body,
        text_width,
        3,
        0,
    )?;

    let mut cursor_top = after_hint - i32::from(body.line_height()) + 12;
    if ready {
        draw_portal_card(display, state, cursor_top, via_hotspot)?;
        cursor_top += PORTAL_CARD_HEIGHT + 24;
        Rectangle::new(
            Point::new(CONTENT_LEFT, cursor_top),
            Size::new(CONTENT_WIDTH as u32, 1),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(display)?;
        cursor_top += 16;
    }

    // What the portal is doing, wrapped: a file name or a join error can be
    // far wider than the screen.
    let mut baseline = cursor_top + i32::from(body.line_height()) + 4;
    if ready && via_hotspot {
        draw_status_dot(
            display,
            CONTENT_LEFT,
            baseline,
            matches!(transfer.join, JoinAttemptState::Succeeded { .. }),
        )?;
        baseline = draw_paragraph(
            display,
            &provision_join_status_text(&transfer.join, locale),
            CONTENT_LEFT + 26,
            baseline,
            body,
            CONTENT_WIDTH - 26,
            2,
            2,
        )?;
    } else if ready {
        baseline = draw_paragraph(
            display,
            &wifi_transfer_status_text(transfer, locale),
            CONTENT_LEFT,
            baseline,
            body,
            CONTENT_WIDTH,
            2,
            2,
        )?;
    }
    if let Some(error) = transfer.error.as_deref() {
        baseline = draw_paragraph(
            display,
            error,
            CONTENT_LEFT,
            baseline,
            detail,
            CONTENT_WIDTH,
            3,
            2,
        )?;
    }

    let button_top = baseline - i32::from(body.line_height()) + 14;
    let button_label = match (transfer.state, via_hotspot) {
        (WifiTransferState::Off | WifiTransferState::Failed, _) => t(locale, "Back", "Indietro"),
        (_, true) => t(locale, "Cancel setup", "Annulla configurazione"),
        (_, false) => t(locale, "Stop and go back", "Ferma e torna indietro"),
    };
    draw_row_frame(display, button_top, PORTAL_BUTTON_HEIGHT, true)?;
    draw_text_centered(
        display,
        button_label,
        CONTENT_LEFT,
        CONTENT_WIDTH,
        centered_baseline(heading, button_top, PORTAL_BUTTON_HEIGHT),
        heading,
    )?;
    draw_footer(
        display,
        state,
        &footer_hints(
            locale,
            &[
                (FooterKey::Select, t(locale, "STOP", "FERMA")),
                (FooterKey::Boot, back_action(locale)),
            ],
        ),
    )
}

/// The QR code and the address, code and folder (LAN) or the hotspot's name
/// and password beside it.
fn draw_portal_card(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    card_top: i32,
    via_hotspot: bool,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let transfer = &state.wifi_transfer;

    RoundedRectangle::new(
        Rectangle::new(
            Point::new(CONTENT_LEFT, card_top),
            Size::new(CONTENT_WIDTH as u32, PORTAL_CARD_HEIGHT as u32),
        ),
        CornerRadii::new(PORTAL_CARD_CORNER_RADIUS),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
    .draw(display)?;

    let qr_left = CONTENT_LEFT + PORTAL_CARD_PAD;
    let text_left = qr_left + PORTAL_QR_BOX + PORTAL_CARD_PAD;
    let text_width = CONTENT_RIGHT - PORTAL_CARD_PAD - text_left;
    let detail_top = card_top + PORTAL_CARD_PAD + 20;
    let qr_top = card_top + (PORTAL_CARD_HEIGHT - PORTAL_QR_BOX) / 2;

    let ap_ssid = transfer.ap_ssid.as_deref().unwrap_or("--");
    let ap_password = transfer.ap_password.as_deref().unwrap_or("--");
    let payload = if via_hotspot {
        Some(crate::app::widgets::qr::wifi_join_payload(
            ap_ssid,
            ap_password,
        ))
    } else {
        transfer.url.clone()
    };
    if let Some(payload) = payload.as_deref() {
        let (module_px, qr_inner_left) = match crate::app::widgets::qr::qr_modules_wide(payload) {
            Some(modules) if modules > 0 => {
                let module_px = (PORTAL_QR_BOX / modules as i32).clamp(2, 7);
                let qr_inner_left =
                    qr_left + ((PORTAL_QR_BOX - modules as i32 * module_px) / 2).max(0);
                (module_px, qr_inner_left)
            }
            _ => (3, qr_left),
        };
        draw_qr(
            display,
            Point::new(qr_inner_left, qr_top),
            module_px,
            payload,
        )?;
    }

    if via_hotspot {
        let network_line = format!("{}: {ap_ssid}", t(locale, "Network", "Rete"));
        draw_text_fit(
            display,
            &network_line,
            Point::new(text_left, detail_top),
            heading,
            text_width,
        )?;
        Text::new("Password:", Point::new(text_left, detail_top + 30), body).draw(display)?;
        draw_text_fit(
            display,
            ap_password,
            Point::new(text_left, detail_top + 60),
            heading,
            text_width,
        )?;
    } else {
        // The address is all the browser needs: no code to type.
        Text::new(
            t(locale, "Address", "Indirizzo"),
            Point::new(text_left, detail_top),
            body,
        )
        .draw(display)?;
        draw_text_fit(
            display,
            &bare_host_label(transfer.url.as_deref()),
            Point::new(text_left, detail_top + 32),
            heading,
            text_width,
        )?;
        let folder_line = format!("{}: /RUSTMIX", t(locale, "Folder", "Cartella"));
        draw_text_fit(
            display,
            &folder_line,
            Point::new(text_left, detail_top + 80),
            body,
            text_width,
        )?;
    }
    Ok(())
}

/// Locale-aware headline shown beside the status dot: unlike
/// `WifiTransferState::label_i18n`, which stays terse for the diagnostics
/// table other screens use, this reads as a short sentence matching the
/// portal's mock.
fn wifi_transfer_status_label(state: WifiTransferState, locale: Locale) -> &'static str {
    match state {
        WifiTransferState::Off => t(locale, "Portal stopped", "Portale fermo"),
        WifiTransferState::Starting => t(locale, "Starting...", "Avvio in corso..."),
        WifiTransferState::Ready => t(locale, "Ready to connect", "Pronto per la connessione"),
        WifiTransferState::Failed => t(locale, "Could not start", "Avvio non riuscito"),
    }
}

/// What to do next, shown under the headline: which of the two ways in the
/// portal is using while it runs (driven by `via_hotspot`), and that it is
/// not reachable when it does not.
fn wifi_transfer_mode_hint(
    state: WifiTransferState,
    via_hotspot: bool,
    locale: Locale,
) -> &'static str {
    match (state, via_hotspot) {
        (WifiTransferState::Off, _) => t(
            locale,
            "The transfer page is not reachable now.",
            "La pagina di trasferimento ora non \u{00E8} raggiungibile.",
        ),
        (WifiTransferState::Starting, _) => t(
            locale,
            "Getting the transfer page ready.",
            "Preparazione della pagina di trasferimento.",
        ),
        (WifiTransferState::Failed, _) => t(
            locale,
            "Go back and try again.",
            "Torna indietro e riprova.",
        ),
        (WifiTransferState::Ready, true) => t(
            locale,
            "No network available: join the device's network to upload files or set up Wi-Fi.",
            "Nessuna rete disponibile: collegati alla rete del dispositivo per caricare file o configurare il Wi-Fi.",
        ),
        (WifiTransferState::Ready, false) => t(
            locale,
            "Open the address in a browser on the same network to upload files. Its Wi-Fi tab adds networks.",
            "Apri l'indirizzo da un browser sulla stessa rete per caricare file. La scheda Wi-Fi aggiunge reti.",
        ),
    }
}

/// The portal URL stripped down to the bare address (e.g. `192.168.1.10`)
/// for the compact card layout; the scheme and trailing slash are implied.
fn bare_host_label(url: Option<&str>) -> String {
    match url {
        Some(url) => url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string(),
        None => "--".into(),
    }
}

/// What the portal last did, in the user's language. The portal records its
/// actions in English for the serial log and its own web page; the fixed
/// ones and the verbs are translated here, the file name is kept.
fn wifi_transfer_status_text(transfer: &WifiTransferSnapshot, locale: Locale) -> String {
    let action = transfer.last_action.as_str();
    if action == "Portal ready" {
        return t(locale, "Waiting for a file...", "In attesa di un file...").to_string();
    }
    if locale == Locale::English {
        return action.to_string();
    }
    const FIXED: [(&str, &str); 4] = [
        ("Portal is off", "Portale fermo"),
        ("Starting portal", "Avvio del portale"),
        ("Portal start failed", "Avvio del portale non riuscito"),
        ("Listed books", "Elenco dei libri letto"),
    ];
    if let Some((_, italian)) = FIXED.iter().find(|(english, _)| *english == action) {
        return (*italian).to_string();
    }
    const VERBS: [(&str, &str); 7] = [
        ("Listed ", "Elenco letto: "),
        ("Downloaded ", "Scaricato: "),
        ("Uploaded ", "Caricato: "),
        ("Deleted ", "Eliminato: "),
        ("Created ", "Creato: "),
        ("Renamed ", "Rinominato: "),
        ("Cover ", "Copertina: "),
    ];
    for (english, italian) in VERBS {
        if let Some(rest) = action.strip_prefix(english) {
            return format!("{italian}{rest}");
        }
    }
    action.to_string()
}

/// Filled when `ready`, hollow otherwise, sitting just left of the status
/// headline on the same baseline (mirrors `screens::reader::draw_dot`'s
/// bullet placement).
fn draw_status_dot(
    display: &mut OrientedFrameBuffer<'_>,
    left: i32,
    baseline: i32,
    ready: bool,
) -> Result<(), Infallible> {
    let style = if ready {
        PrimitiveStyle::with_fill(BinaryColor::On)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 2)
    };
    Circle::new(
        Point::new(left, baseline - PORTAL_DOT_DIAMETER),
        PORTAL_DOT_DIAMETER as u32,
    )
    .into_styled(style)
    .draw(display)
}

pub fn render_network_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    let network = &state.network;
    let zone = state.regional.timezone_label_for_rtc(state.board.rtc);

    draw_header(display, state, t(locale, "NETWORK INFO", "INFO RETE"))?;

    let mut baseline = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        t(locale, "Configuration", "Configurazione"),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "File", "File"),
        NetworkSnapshot::config_path(),
    )?;
    baseline = draw_paragraph(
        display,
        t(
            locale,
            "Networks are added from the phone. The file can also be edited on the SD card; restart afterwards.",
            "Le reti si aggiungono dal telefono. Il file si pu\u{00F2} anche modificare sulla scheda SD; poi riavvia.",
        ),
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        4,
        4,
    )?;

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Date and time", "Data e ora"),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Time zone", "Fuso orario"),
        &zone,
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "NTP server", "Server NTP"),
        &network.ntp_server,
    )?;

    baseline = draw_section_title(
        display,
        preferences,
        baseline + 22,
        t(locale, "Last error", "Ultimo errore"),
    )?;
    draw_paragraph(
        display,
        network
            .error
            .as_deref()
            .unwrap_or(t(locale, "None", "Nessuno")),
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        5,
        4,
    )?;
    draw_footer(display, state, &back_only(locale))
}

#[cfg(test)]
mod tests {
    use super::{
        render_network, render_network_details, render_network_saved, render_wifi_transfer,
    };
    use crate::{app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer};

    #[test]
    fn network_overview_details_transfer_provision_and_saved_render_without_configuration() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        render_network(&mut display, &state).unwrap();
        render_network_details(&mut display, &state).unwrap();
        render_wifi_transfer(&mut display, &state).unwrap();
        render_network_saved(&mut display, &state).unwrap();

        state.wifi_transfer = crate::wifi_transfer::WifiTransferSnapshot {
            state: crate::wifi_transfer::WifiTransferState::Ready,
            ap_ssid: Some("RUSTMIX-1234".into()),
            ap_password: Some("AB3F7K9QXZ2M".into()),
            join: crate::wifi_transfer::JoinAttemptState::Testing {
                ssid: "Lab WiFi".into(),
            },
            ..Default::default()
        };
        render_wifi_transfer(&mut display, &state).unwrap();

        state.wifi_transfer.join = crate::wifi_transfer::JoinAttemptState::Failed {
            ssid: "Lab WiFi".into(),
            error: "wrong password".into(),
        };
        render_wifi_transfer(&mut display, &state).unwrap();

        state.set_saved_networks(vec![crate::network_saved::SavedNetworkEntry {
            ssid: "Lab WiFi".into(),
            connected: true,
        }]);
        render_network_saved(&mut display, &state).unwrap();
    }
}
