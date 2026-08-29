//! Wi-Fi, SNTP, phone Wi-Fi provisioning and explicitly activated SD-card
//! transfer portal screens.

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
        widgets::{footer::draw_footer, header::draw_header, qr::draw_qr},
    },
    network::NetworkSnapshot,
    network_provision::{JoinAttemptState, NetworkProvisionState},
    orientation::OrientedFrameBuffer,
};

pub fn render_network(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let network = &state.network;
    let rssi = network.rssi_label();

    draw_header(display, state, "NETWORK")?;

    Text::new("Wi-Fi station", Point::new(22, 112), heading).draw(display)?;
    line(display, 160, "State", network.wifi_state.label(), body)?;
    line(display, 200, "SSID", network.ssid_label(), body)?;
    line(display, 240, "IPv4", network.ipv4_label(), body)?;
    line(display, 280, "RSSI", &rssi, body)?;
    line(
        display,
        320,
        "Saved",
        &format!("{} network(s)", network.saved_network_count),
        body,
    )?;

    Text::new("Actions", Point::new(22, 380), heading).draw(display)?;
    draw_action(
        display,
        424,
        "Configure via phone",
        state.network_action_selected == 0,
        body,
    )?;
    draw_action(
        display,
        492,
        "Saved networks",
        state.network_action_selected == 1,
        body,
    )?;
    draw_action(
        display,
        560,
        "Provisioning details",
        state.network_action_selected == 2,
        body,
    )?;
    draw_footer(
        display,
        state.display,
        "UP DOWN MOVE  SELECT OPEN  BOOT BACK",
    )?;
    Ok(())
}

/// Phone Wi-Fi provisioning screen: shows the device's own hotspot and the
/// portal address as text and as a single scannable QR code that auto-joins
/// the hotspot, plus the live outcome of whatever the phone is doing. The
/// captive-portal DNS hijack (see `crate::dns_captive_portal`) gets the
/// phone's OS to open the portal page on its own after joining, so a second
/// "open this URL" QR code is no longer needed. Replaces the old on-device
/// rotary-keyboard SSID/password editor.
pub fn render_network_provision(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let provision = &state.network_provision;

    draw_header(display, state, "WI-FI SETUP")?;

    match provision.state {
        NetworkProvisionState::Off | NetworkProvisionState::Starting => {
            Text::new("Starting hotspot...", Point::new(22, 130), heading).draw(display)?;
        }
        NetworkProvisionState::Failed => {
            Text::new("Could not start Wi-Fi setup.", Point::new(22, 118), heading)
                .draw(display)?;
            Text::new(
                provision.error.as_deref().unwrap_or("Unknown error"),
                Point::new(22, 154),
                detail,
            )
            .draw(display)?;
        }
        NetworkProvisionState::Ready => {
            let ap_ssid = provision.ap_ssid.as_deref().unwrap_or("--");
            let ap_password = provision.ap_password.as_deref().unwrap_or("--");
            let portal_url = provision.portal_url.as_deref().unwrap_or("--");

            Text::new("On your phone", Point::new(22, 106), heading).draw(display)?;
            line(display, 138, "Hotspot", ap_ssid, body)?;
            line(display, 172, "Password", ap_password, body)?;
            line(display, 206, "Opens", portal_url, detail)?;

            Text::new("Scan to connect", Point::new(22, 250), body).draw(display)?;
            let join_payload = crate::app::widgets::qr::wifi_join_payload(ap_ssid, ap_password);
            let status_top = 470;
            // Screen is 480px wide in portrait (see
            // `DisplayOrientation::Portrait::logical_size`); size the module
            // to fill the box between the label above and "Status" below
            // regardless of QR version, then center it, falling back to a
            // small fixed code at the left margin if it somehow can't be
            // encoded.
            let qr_box_height = (status_top - 268) as u32;
            let qr_box_width = 480 - 2 * 22;
            let (module_px, qr_left) =
                match crate::app::widgets::qr::qr_modules_wide(&join_payload) {
                    Some(modules) if modules > 0 => {
                        let module_px = (qr_box_height / modules)
                            .min(qr_box_width / modules)
                            .clamp(3, 9) as i32;
                        let qr_left = ((480 - modules as i32 * module_px) / 2).max(22);
                        (module_px, qr_left)
                    }
                    _ => (4, 22),
                };
            draw_qr(display, Point::new(qr_left, 268), module_px, &join_payload)?;

            Text::new("Status", Point::new(22, status_top), heading).draw(display)?;
            let (label, detail_line) = join_status_labels(&provision.join);
            line(display, status_top + 42, "Phone", label, body)?;
            if let Some(extra) = detail_line {
                Text::new(&extra, Point::new(22, status_top + 82), detail).draw(display)?;
            }
        }
    }

    draw_footer(display, state.display, "SELECT STOP  BOOT STOP + BACK")?;
    Ok(())
}

fn join_status_labels(join: &JoinAttemptState) -> (&'static str, Option<String>) {
    match join {
        JoinAttemptState::Idle => ("Waiting for a network", None),
        JoinAttemptState::Testing { ssid } => ("Connecting", Some(ssid.clone())),
        JoinAttemptState::Succeeded { ssid } => ("Connected and saved", Some(ssid.clone())),
        JoinAttemptState::Failed { ssid, error } => {
            ("Failed", Some(format!("{ssid}: {error}")))
        }
    }
}

/// Read-only saved-network list: no typing, just view and forget. Adding a
/// network or changing its password happens through the phone portal.
pub fn render_network_saved(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let detail = state.display.detail_style();
    let saved = &state.network_saved;

    draw_header(display, state, "SAVED NETWORKS")?;

    if saved.networks.is_empty() {
        Text::new("No networks saved yet.", Point::new(22, 130), heading).draw(display)?;
        Text::new(
            "Use Configure via phone to add one.",
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
                Text::new("CONNECTED", Point::new(330, top + 32), detail).draw(display)?;
            }
        }
        if saved.confirming_forget {
            Text::new(
                "SELECT again to forget this network.",
                Point::new(22, 700),
                detail,
            )
            .draw(display)?;
        }
    }

    draw_footer(display, state.display, "MOVE  SELECT FORGET  BOOT BACK")?;
    Ok(())
}

pub fn render_wifi_transfer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let transfer = &state.wifi_transfer;

    draw_header(display, state, "WI-FI TRANSFER")?;

    Text::new("Portal status", Point::new(22, 118), heading).draw(display)?;
    line(display, 166, "State", transfer.state.label(), body)?;
    line(display, 206, "Open", transfer.url_label(), detail)?;
    line(display, 246, "Code", transfer.code_label(), body)?;
    line(display, 286, "Root", "/RUSTMIX", body)?;

    // Same "label above a centered, self-sized code" layout as the Wi-Fi
    // provisioning QR in `render_network_provision`: scanning opens the
    // portal page directly, then the visitor still types the `Code` shown
    // above (the root page itself needs no auth, only the `/api/*` routes
    // do; see `wifi_transfer::authenticate`).
    let last_request_top = 520;
    match transfer.url.as_deref() {
        Some(url) => {
            Text::new("Scan to open", Point::new(22, 326), body).draw(display)?;
            let qr_top = 350;
            let qr_box_height = (last_request_top - qr_top) as u32;
            let qr_box_width = 480 - 2 * 22;
            let (module_px, qr_left) = match crate::app::widgets::qr::qr_modules_wide(url) {
                Some(modules) if modules > 0 => {
                    let module_px = (qr_box_height / modules)
                        .min(qr_box_width / modules)
                        .clamp(3, 9) as i32;
                    let qr_left = ((480 - modules as i32 * module_px) / 2).max(22);
                    (module_px, qr_left)
                }
                _ => (4, 22),
            };
            draw_qr(display, Point::new(qr_left, qr_top), module_px, url)?;
        }
        None => {
            Text::new("Portal not ready yet.", Point::new(22, 326), detail).draw(display)?;
        }
    }

    Text::new("Last request", Point::new(22, last_request_top), heading).draw(display)?;
    Text::new(&transfer.last_action, Point::new(22, last_request_top + 42), detail)
        .draw(display)?;
    let bytes = format!("{} bytes", transfer.last_bytes);
    Text::new(&bytes, Point::new(22, last_request_top + 82), detail).draw(display)?;
    if let Some(error) = transfer.error.as_deref() {
        Text::new(error, Point::new(22, last_request_top + 122), detail).draw(display)?;
    }

    draw_action(display, last_request_top + 162, "Stop and return", true, body)?;
    draw_footer(display, state.display, "SELECT STOP  BOOT STOP + BACK")?;
    Ok(())
}

pub fn render_network_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let network = &state.network;
    let zone = state.regional.timezone_label_for_rtc(state.board.rtc);
    let error = network.error.as_deref().unwrap_or("none");

    draw_header(display, state, "NETWORK DETAILS")?;

    Text::new("Configuration file", Point::new(22, 118), heading).draw(display)?;
    Text::new(NetworkSnapshot::config_path(), Point::new(22, 166), body).draw(display)?;
    Text::new(
        "Configure via phone, or edit this file",
        Point::new(22, 206),
        body,
    )
    .draw(display)?;
    Text::new("on the SD card and reboot.", Point::new(22, 240), body).draw(display)?;

    Text::new("Regional settings", Point::new(22, 300), heading).draw(display)?;
    line(display, 348, "Timezone", &zone, body)?;
    line(
        display,
        388,
        "RTC storage",
        &state.regional.rtc_storage_label(),
        body,
    )?;
    line(display, 428, "NTP server", &network.ntp_server, body)?;

    Text::new("Last error", Point::new(22, 500), heading).draw(display)?;
    Text::new(error, Point::new(22, 540), detail).draw(display)?;
    draw_footer(display, state.display, "BOOT BACK")?;
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
    use super::{
        render_network, render_network_details, render_network_provision, render_network_saved,
        render_wifi_transfer,
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
        render_network_provision(&mut display, &state).unwrap();
        render_network_saved(&mut display, &state).unwrap();

        state.network_provision = crate::network_provision::NetworkProvisionSnapshot {
            state: crate::network_provision::NetworkProvisionState::Ready,
            ap_ssid: Some("RUSTMIX-1234".into()),
            ap_password: Some("AB3F7K9QXZ2M".into()),
            portal_url: Some("http://192.168.71.1/".into()),
            join: crate::network_provision::JoinAttemptState::Testing {
                ssid: "Lab WiFi".into(),
            },
            saved_network_count: 1,
            error: None,
        };
        render_network_provision(&mut display, &state).unwrap();

        state
            .network_provision
            .join = crate::network_provision::JoinAttemptState::Failed {
            ssid: "Lab WiFi".into(),
            error: "wrong password".into(),
        };
        render_network_provision(&mut display, &state).unwrap();

        state.set_saved_networks(vec![crate::network_saved::SavedNetworkEntry {
            ssid: "Lab WiFi".into(),
            connected: true,
        }]);
        render_network_saved(&mut display, &state).unwrap();
    }
}
