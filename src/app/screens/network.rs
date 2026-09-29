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
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header, qr::draw_qr},
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
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let network = &state.network;
    let rssi = network.rssi_label();

    draw_header(display, state, t(locale, "NETWORK", "RETE"))?;

    Text::new(
        t(locale, "Wi-Fi station", "Stazione Wi-Fi"),
        Point::new(22, 112),
        heading,
    )
    .draw(display)?;
    line(
        display,
        160,
        t(locale, "State", "Stato"),
        network.wifi_state.label_i18n(locale),
        body,
    )?;
    line(display, 200, "SSID", network.ssid_label(), body)?;
    line(display, 240, "IPv4", network.ipv4_label(), body)?;
    line(display, 280, "RSSI", &rssi, body)?;
    let saved = match locale {
        Locale::English => format!("{} network(s)", network.saved_network_count),
        Locale::Italian => format!("{} rete/i", network.saved_network_count),
    };
    line(display, 320, t(locale, "Saved", "Salvate"), &saved, body)?;

    Text::new(t(locale, "Actions", "Azioni"), Point::new(22, 380), heading).draw(display)?;
    draw_action(
        display,
        424,
        t(locale, "Configure via phone", "Configura da telefono"),
        state.network_action_selected == 0,
        body,
    )?;
    draw_action(
        display,
        492,
        t(locale, "Saved networks", "Reti salvate"),
        state.network_action_selected == 1,
        body,
    )?;
    draw_action(
        display,
        560,
        t(locale, "Provisioning details", "Dettagli configurazione"),
        state.network_action_selected == 2,
        body,
    )?;
    Ok(())
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

/// Read-only saved-network list: no typing, just view and forget. Adding a
/// network or changing its password happens through the phone portal.
pub fn render_network_saved(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let detail = state.display.detail_style();
    let saved = &state.network_saved;

    draw_header(display, state, t(locale, "SAVED WI-FI", "RETI SALVATE"))?;

    if saved.networks.is_empty() {
        Text::new(
            t(locale, "No networks saved yet.", "Nessuna rete salvata."),
            Point::new(22, 130),
            heading,
        )
        .draw(display)?;
        Text::new(
            t(
                locale,
                "Use Configure via phone to add one.",
                "Usa Configura da telefono per aggiungerne una.",
            ),
            Point::new(22, 166),
            detail,
        )
        .draw(display)?;
    } else {
        let selected_on_page = saved.selected_on_page();
        for (index, entry) in saved.visible_entries().iter().enumerate() {
            let top = 108 + (index as i32 * 66);
            let selected = index == selected_on_page;
            let outline = if selected {
                PrimitiveStyle::with_stroke(BinaryColor::On, 3)
            } else {
                PrimitiveStyle::with_stroke(BinaryColor::On, 1)
            };
            Rectangle::new(Point::new(22, top), Size::new(436, 54))
                .into_styled(outline)
                .draw(display)?;
            Text::new(
                if selected { ">" } else { " " },
                Point::new(36, top + 32),
                heading,
            )
            .draw(display)?;
            Text::new(
                &placeholder_or_truncate(&entry.ssid, 22),
                Point::new(62, top + 32),
                heading,
            )
            .draw(display)?;
            if entry.connected {
                Text::new(
                    t(locale, "CONNECTED", "CONNESSA"),
                    Point::new(330, top + 32),
                    detail,
                )
                .draw(display)?;
            }
        }
        if saved.confirming_forget {
            Text::new(
                t(
                    locale,
                    "SELECT again to forget this network.",
                    "Premi di nuovo SELECT per dimenticare questa rete.",
                ),
                Point::new(22, 700),
                detail,
            )
            .draw(display)?;
        }
    }

    draw_footer(
        display,
        state,
        t(locale, "SELECT FORGET", "SELECT DIMENTICA"),
    )?;
    Ok(())
}

/// Left/right margin shared by the status card, divider and stop button.
const PORTAL_CONTENT_LEFT: i32 = 22;
const PORTAL_CONTENT_RIGHT: i32 = 458;
/// Inner padding of the status card holding the QR code and connection
/// details.
const PORTAL_CARD_PAD: i32 = 20;
/// Target side length of the QR module, vertically centered in the card;
/// the actual drawn size still self-sizes to a whole number of modules.
const PORTAL_QR_BOX: i32 = 140;
const PORTAL_CARD_CORNER_RADIUS: Size = Size::new(14, 14);
const PORTAL_DOT_DIAMETER: i32 = 12;

pub fn render_wifi_transfer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let transfer = &state.wifi_transfer;
    // No Wi-Fi joined yet: the portal was bootstrapped from the device's own
    // hotspot (see `NetworkRuntime::start_provisioning`) instead of the
    // already-connected LAN, so the QR below joins that hotspot first.
    let via_hotspot = transfer.ap_ssid.is_some();

    draw_header(display, state, t(locale, "UPLOAD", "CARICA"))?;

    let status_baseline = Point::new(PORTAL_CONTENT_LEFT + 18, 96);
    draw_status_dot(
        display,
        PORTAL_CONTENT_LEFT,
        status_baseline.y,
        transfer.state == WifiTransferState::Ready,
    )?;
    Text::new(
        wifi_transfer_status_label(transfer.state, locale),
        status_baseline,
        heading,
    )
    .draw(display)?;

    let hint_left = PORTAL_CONTENT_LEFT + 18;
    let hint_width = PORTAL_CONTENT_RIGHT - hint_left;
    let mut hint_baseline_y = status_baseline.y + i32::from(heading.line_height());
    for line in wrap_to_width(
        body,
        wifi_transfer_mode_hint(via_hotspot, locale),
        hint_width,
    ) {
        Text::new(&line, Point::new(hint_left, hint_baseline_y), body).draw(display)?;
        hint_baseline_y += i32::from(body.line_height());
    }

    let card_top = hint_baseline_y + 12;
    let card_height = 216;
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(PORTAL_CONTENT_LEFT, card_top),
            Size::new(
                (PORTAL_CONTENT_RIGHT - PORTAL_CONTENT_LEFT) as u32,
                card_height as u32,
            ),
        ),
        CornerRadii::new(PORTAL_CARD_CORNER_RADIUS),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 2))
    .draw(display)?;

    let qr_left = PORTAL_CONTENT_LEFT + PORTAL_CARD_PAD;
    let text_left = qr_left + PORTAL_QR_BOX + PORTAL_CARD_PAD;
    let detail_top = card_top + PORTAL_CARD_PAD + 20;

    if via_hotspot {
        let ap_ssid = transfer.ap_ssid.as_deref().unwrap_or("--");
        let ap_password = transfer.ap_password.as_deref().unwrap_or("--");
        let join_payload = crate::app::widgets::qr::wifi_join_payload(ap_ssid, ap_password);
        let (module_px, qr_inner_left) =
            match crate::app::widgets::qr::qr_modules_wide(&join_payload) {
                Some(modules) if modules > 0 => {
                    let module_px = (PORTAL_QR_BOX / modules as i32).clamp(2, 7);
                    let qr_inner_left =
                        qr_left + ((PORTAL_QR_BOX - modules as i32 * module_px) / 2).max(0);
                    (module_px, qr_inner_left)
                }
                _ => (3, qr_left),
            };
        let qr_top = card_top + (card_height - PORTAL_QR_BOX) / 2;
        draw_qr(
            display,
            Point::new(qr_inner_left, qr_top),
            module_px,
            &join_payload,
        )?;

        let network_line = format!("{}: {ap_ssid}", t(locale, "Network", "Rete"));
        Text::new(&network_line, Point::new(text_left, detail_top), heading).draw(display)?;
        Text::new(
            t(locale, "Password:", "Password:"),
            Point::new(text_left, detail_top + 30),
            body,
        )
        .draw(display)?;
        Text::new(ap_password, Point::new(text_left, detail_top + 60), heading).draw(display)?;
        let code_line = format!("{}: {}", t(locale, "Code", "Codice"), transfer.code_label());
        Text::new(&code_line, Point::new(text_left, detail_top + 94), body).draw(display)?;
    } else {
        match transfer.url.as_deref() {
            Some(url) => {
                let (module_px, qr_inner_left) =
                    match crate::app::widgets::qr::qr_modules_wide(url) {
                        Some(modules) if modules > 0 => {
                            let module_px = (PORTAL_QR_BOX / modules as i32).clamp(2, 7);
                            let qr_inner_left = qr_left
                                + ((PORTAL_QR_BOX - modules as i32 * module_px) / 2).max(0);
                            (module_px, qr_inner_left)
                        }
                        _ => (3, qr_left),
                    };
                let qr_top = card_top + (card_height - PORTAL_QR_BOX) / 2;
                draw_qr(display, Point::new(qr_inner_left, qr_top), module_px, url)?;
            }
            None => {
                Text::new(
                    t(
                        locale,
                        "Portal not ready yet.",
                        "Portale non ancora pronto.",
                    ),
                    Point::new(qr_left, detail_top),
                    detail,
                )
                .draw(display)?;
            }
        }

        Text::new(
            &bare_host_label(transfer.url.as_deref()),
            Point::new(text_left, detail_top),
            body,
        )
        .draw(display)?;
        let code_line = format!("{}: {}", t(locale, "Code", "Codice"), transfer.code_label());
        Text::new(&code_line, Point::new(text_left, detail_top + 40), heading).draw(display)?;
        let folder_line = format!("{}: /RUSTMIX", t(locale, "Folder", "Cartella"));
        Text::new(&folder_line, Point::new(text_left, detail_top + 80), body).draw(display)?;
    }

    let divider_top = card_top + card_height + 24;
    Rectangle::new(
        Point::new(PORTAL_CONTENT_LEFT, divider_top),
        Size::new((PORTAL_CONTENT_RIGHT - PORTAL_CONTENT_LEFT) as u32, 1),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
    .draw(display)?;

    let section_top = divider_top + 36;
    if via_hotspot {
        draw_status_dot(
            display,
            PORTAL_CONTENT_LEFT,
            section_top + 6,
            matches!(transfer.join, JoinAttemptState::Succeeded { .. }),
        )?;
        Text::new(
            &provision_join_status_text(&transfer.join, locale),
            Point::new(PORTAL_CONTENT_LEFT + 26, section_top + 6),
            body,
        )
        .draw(display)?;
    } else {
        Text::new(
            t(locale, "TRANSFER STATUS", "STATO TRASFERIMENTO"),
            Point::new(PORTAL_CONTENT_LEFT, section_top),
            detail,
        )
        .draw(display)?;
        Text::new(
            &wifi_transfer_status_text(transfer, locale),
            Point::new(PORTAL_CONTENT_LEFT, section_top + 38),
            body,
        )
        .draw(display)?;
    }
    let mut button_top = section_top + 78;
    if let Some(error) = transfer.error.as_deref() {
        Text::new(error, Point::new(PORTAL_CONTENT_LEFT, button_top), detail).draw(display)?;
        button_top += 40;
    }

    draw_portal_button(
        display,
        button_top,
        if via_hotspot {
            t(locale, "Cancel setup", "Annulla configurazione")
        } else {
            t(locale, "Stop and return", "Ferma e torna indietro")
        },
        heading,
    )?;
    draw_footer(
        display,
        state,
        t(locale, "SELECT STOP  BOOT STOP", "SELECT FERMA  BOOT FERMA"),
    )?;
    Ok(())
}

/// Locale-aware headline shown beside the status dot: unlike
/// `WifiTransferState::label_i18n`, which stays terse for the diagnostics
/// table other screens use, this reads as a short sentence matching the
/// portal's mock.
fn wifi_transfer_status_label(state: WifiTransferState, locale: Locale) -> &'static str {
    match state {
        WifiTransferState::Off => t(locale, "Transfer stopped", "Trasferimento fermato"),
        WifiTransferState::Starting => t(locale, "Starting...", "Avvio in corso..."),
        WifiTransferState::Ready => t(locale, "Ready to connect", "Pronto per la connessione"),
        WifiTransferState::Failed => t(locale, "Could not start", "Avvio non riuscito"),
    }
}

/// Greedy pixel-width word-wrap with no line cap, since the mode hint below
/// is short enough that wrapping ever needing more than two lines would mean
/// the copy itself needs trimming.
fn wrap_to_width(style: UiTextStyle, text: &str, max_width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && style.text_width(&candidate) > max_width {
            lines.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// One-line explanation of why the portal is in hotspot vs. direct-transfer
/// mode, shown under the ready/status headline so the mode switch (driven by
/// `via_hotspot`, see the comment above) doesn't read as unexplained.
fn wifi_transfer_mode_hint(via_hotspot: bool, locale: Locale) -> &'static str {
    if via_hotspot {
        t(
            locale,
            "No network available — join the device network to upload",
            "Nessuna rete disponibile — collegati alla rete del dispositivo per caricare",
        )
    } else {
        t(
            locale,
            "Connected to your network — open the address to upload files",
            "Connesso alla tua rete — apri l'indirizzo per caricare i file",
        )
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

/// Body line under "TRANSFER STATUS": a friendly idle message before the
/// first request, then the raw diagnostic action text once one arrives (same
/// untranslated strings the previous "Last request" row showed).
fn wifi_transfer_status_text(transfer: &WifiTransferSnapshot, locale: Locale) -> String {
    if transfer.state == WifiTransferState::Ready && transfer.last_action == "Portal ready" {
        t(locale, "Waiting for a file...", "In attesa di un file...").to_string()
    } else {
        transfer.last_action.clone()
    }
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

/// Bold rounded-rectangle button spanning the same width as the status
/// card, with its label centered both ways.
fn draw_portal_button(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let width = PORTAL_CONTENT_RIGHT - PORTAL_CONTENT_LEFT;
    let height = 64;
    RoundedRectangle::new(
        Rectangle::new(
            Point::new(PORTAL_CONTENT_LEFT, top),
            Size::new(width as u32, height as u32),
        ),
        CornerRadii::new(PORTAL_CARD_CORNER_RADIUS),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 3))
    .draw(display)?;

    let text_width = style.text_width(label);
    let (ink_top, ink_bottom) = style.text_ink_bounds(label);
    let text_left = PORTAL_CONTENT_LEFT + (width - text_width) / 2;
    let baseline = top + (height - (ink_bottom - ink_top)) / 2 - ink_top;
    Text::new(label, Point::new(text_left, baseline), style).draw(display)?;
    Ok(())
}

pub fn render_network_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let network = &state.network;
    let zone = state.regional.timezone_label_for_rtc(state.board.rtc);
    let error = network
        .error
        .as_deref()
        .unwrap_or(t(locale, "none", "nessuno"));

    draw_header(
        display,
        state,
        t(locale, "NETWORK INFO", "INFO RETE"),
    )?;

    Text::new(
        t(locale, "Configuration file", "File di configurazione"),
        Point::new(22, 118),
        heading,
    )
    .draw(display)?;
    Text::new(NetworkSnapshot::config_path(), Point::new(22, 166), body).draw(display)?;
    Text::new(
        t(
            locale,
            "Configure via phone, or edit this file",
            "Configura da telefono, oppure modifica questo file",
        ),
        Point::new(22, 206),
        body,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "on the SD card and reboot.",
            "sulla scheda SD e riavvia.",
        ),
        Point::new(22, 240),
        body,
    )
    .draw(display)?;

    Text::new(
        t(locale, "Regional settings", "Impostazioni regionali"),
        Point::new(22, 300),
        heading,
    )
    .draw(display)?;
    line(
        display,
        348,
        t(locale, "Timezone", "Fuso orario"),
        &zone,
        body,
    )?;
    line(
        display,
        388,
        t(locale, "RTC storage", "Memoria RTC"),
        &state.regional.rtc_storage_label(),
        body,
    )?;
    line(
        display,
        428,
        t(locale, "NTP server", "Server NTP"),
        &network.ntp_server,
        body,
    )?;

    Text::new(
        t(locale, "Last error", "Ultimo errore"),
        Point::new(22, 500),
        heading,
    )
    .draw(display)?;
    Text::new(error, Point::new(22, 540), detail).draw(display)?;
    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.into();
    }
    let mut output: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}

fn placeholder_or_truncate(value: &str, max_chars: usize) -> String {
    if value.is_empty() {
        "_".into()
    } else {
        truncate(value, max_chars)
    }
}

fn line(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    label: &str,
    value: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(label, Point::new(22, y), style).draw(display)?;
    Text::new(value, Point::new(176, y), style).draw(display)?;
    Ok(())
}

fn draw_action(
    display: &mut OrientedFrameBuffer<'_>,
    top: i32,
    label: &str,
    selected: bool,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Rectangle::new(Point::new(22, top), Size::new(436, 52))
        .into_styled(if selected {
            PrimitiveStyle::with_stroke(BinaryColor::On, 6)
        } else {
            PrimitiveStyle::with_stroke(BinaryColor::On, 2)
        })
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
    use super::{render_network, render_network_details, render_network_saved, render_wifi_transfer};
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
