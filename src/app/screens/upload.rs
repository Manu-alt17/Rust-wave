//! Upload: choose how to copy files onto the card, over Wi-Fi from a
//! browser or with the USB cable from a computer.
//!
//! Two tiles side by side, as on Home, and under them what the selected
//! one does, so the choice can be made without opening either.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Point, Size},
};
use embedded_iconoir::{
    icons::size48px::{connectivity::Wifi, devices::Laptop},
    prelude::IconoirNewIcon,
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{draw_footer, select_and_back},
            header::draw_header,
            home_tile::{draw_icon_tile, TILE_GAP_X},
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_ROW_TOP},
            list::{draw_field, draw_section_title},
            text::draw_paragraph,
        },
    },
    network::WifiConnectionState,
    orientation::OrientedFrameBuffer,
};

/// The two tiles fill the content width.
const TILE_SIZE: Size = Size::new(((CONTENT_WIDTH - TILE_GAP_X) / 2) as u32, 150);

pub fn render_upload(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    let wifi_selected = state.upload_selected == 0;
    // The same test the portal makes when it starts: an address on a Wi-Fi
    // network serves the page there, otherwise the device opens its hotspot.
    let connected = state.network.wifi_state == WifiConnectionState::Connected
        && state.network.ipv4_address.is_some();

    draw_header(display, state, t(locale, "UPLOAD", "CARICA"))?;

    draw_icon_tile(
        display,
        Point::new(CONTENT_LEFT, FIRST_ROW_TOP),
        TILE_SIZE,
        "Wi-Fi",
        &Wifi::new(BinaryColor::On),
        wifi_selected,
        preferences,
    )?;
    draw_icon_tile(
        display,
        Point::new(
            CONTENT_LEFT + TILE_SIZE.width as i32 + TILE_GAP_X,
            FIRST_ROW_TOP,
        ),
        TILE_SIZE,
        t(locale, "USB cable", "Cavo USB"),
        &Laptop::new(BinaryColor::On),
        !wifi_selected,
        preferences,
    )?;

    let (title, text) = if !wifi_selected {
        (
            t(locale, "The card as a USB disk", "La scheda come disco USB"),
            t(
                locale,
                "Connect the device to a computer with its USB cable: the microSD shows up as a disk, and you copy books into BOOKS and audiobooks into AUDIO. The quickest way for many or large files. The device cannot be used meanwhile, and restarts when done.",
                "Collega il dispositivo al computer col cavo USB: la microSD compare come un disco e copi i libri in BOOKS e gli audiolibri in AUDIO. \u{00C8} il modo pi\u{00F9} rapido per tanti file o file grandi. Nel frattempo il dispositivo non si usa, e alla fine si riavvia.",
            ),
        )
    } else if connected {
        (
            t(locale, "From a browser, no cable", "Dal browser, senza cavo"),
            t(
                locale,
                "The device shows an address and a QR code: open it from a phone or a computer on the same Wi-Fi network to upload books, audiobooks and sleep images and to manage the files.",
                "Il dispositivo mostra un indirizzo e un codice QR: aprilo da un telefono o da un computer sulla stessa rete Wi-Fi per caricare libri, audiolibri e sfondi e gestire i file.",
            ),
        )
    } else {
        (
            t(locale, "From a browser, no cable", "Dal browser, senza cavo"),
            t(
                locale,
                "No Wi-Fi network is connected, so the device opens its own hotspot and shows a QR code: join it from a phone to upload files and, if you like, to add your Wi-Fi network.",
                "Nessuna rete Wi-Fi \u{00E8} collegata, quindi il dispositivo apre un suo hotspot e mostra un codice QR: collegati dal telefono per caricare i file e, se vuoi, aggiungere la tua rete Wi-Fi.",
            ),
        )
    };

    let tiles_bottom = FIRST_ROW_TOP + TILE_SIZE.height as i32;
    let mut baseline = draw_section_title(display, preferences, tiles_bottom + 44, title)?;
    baseline = draw_paragraph(
        display,
        text,
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        9,
        6,
    )?;
    if wifi_selected {
        let network = if connected {
            state.network.ssid_label()
        } else {
            t(locale, "not connected", "non collegata")
        };
        draw_field(
            display,
            preferences,
            baseline + 14,
            t(locale, "Wi-Fi network", "Rete Wi-Fi"),
            network,
        )?;
    }

    draw_footer(
        display,
        state,
        &select_and_back(locale, t(locale, "OPEN", "APRI")),
    )
}

#[cfg(test)]
mod tests {
    use super::render_upload;
    use crate::{
        app::AppState, framebuffer::FrameBuffer, network::WifiConnectionState,
        orientation::OrientedFrameBuffer,
    };

    #[test]
    fn upload_renders_both_choices_with_and_without_a_network() {
        for (selected, wifi_state) in [
            (0, WifiConnectionState::Connected),
            (0, WifiConnectionState::ConfigurationMissing),
            (1, WifiConnectionState::Connected),
        ] {
            let mut frame = FrameBuffer::new_white();
            let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
            let mut state = AppState::default();
            state.upload_selected = selected;
            state.network.wifi_state = wifi_state;
            state.network.ssid = Some("CasaMia".into());
            state.network.ipv4_address = Some("192.168.1.20".into());
            render_upload(&mut display, &state).unwrap();
        }
    }
}
