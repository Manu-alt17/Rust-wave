//! "Connect to PC": the microSD as a USB disk.

use core::convert::Infallible;

use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};
use embedded_iconoir::{icons::size96px::devices::Laptop, prelude::IconoirNewIcon};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{back_action, draw_footer, footer_hints, FooterKey},
            header::draw_header,
            home_tile::draw_iconoir_icon,
            layout::{CONTENT_LEFT, CONTENT_WIDTH},
            text::{draw_paragraph, draw_text_centered},
        },
    },
    orientation::OrientedFrameBuffer,
    usb_disk::UsbDiskPhase,
};

pub fn render_usb_disk(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let large = state.display.large_style();
    let body = state.display.body_style();
    draw_header(display, state, t(locale, "USB CABLE", "VIA CAVO USB"))?;
    draw_iconoir_icon(
        display,
        Point::new((480 - 96) / 2, 84),
        &Laptop::new(BinaryColor::On),
    )?;

    let (title, text, hint) = match &state.usb_disk {
        UsbDiskPhase::Idle => (
            t(locale, "USB disk", "Disco USB"),
            t(
                locale,
                "Connect the device to a computer with its USB cable and press SELECT: the microSD shows up on the computer as a disk, to copy books into BOOKS and audiobooks into AUDIO. The device cannot be used meanwhile. When done, eject the disk on the computer, then press any key here: the device restarts and reads the new files.",
                "Collega il dispositivo al computer col cavo USB e premi SELECT: la microSD compare sul computer come un disco, per copiare i libri in BOOKS e gli audiolibri in AUDIO. Nel frattempo il dispositivo non si usa. Finito, espelli il disco dal computer, poi premi un tasto qui: il dispositivo si riavvia e legge i nuovi file.",
            )
            .to_string(),
            footer_hints(
                locale,
                &[
                    (FooterKey::Select, t(locale, "CONNECT", "COLLEGA")),
                    (FooterKey::Boot, back_action(locale)),
                ],
            ),
        ),
        UsbDiskPhase::Active => (
            t(locale, "Connected", "Collegato"),
            t(
                locale,
                "The computer now sees the microSD as a disk. Copy the files, eject the disk on the computer, then press UP, DOWN or SELECT to restart.",
                "Il computer ora vede la microSD come un disco. Copia i file, espelli il disco dal computer, poi premi SU, GIÙ o SELECT per riavviare.",
            )
            .to_string(),
            restart_hint(locale),
        ),
        UsbDiskPhase::Failed(error) => (
            t(locale, "Not connected", "Collegamento non riuscito"),
            format!(
                "{} {error}",
                t(
                    locale,
                    "Press UP, DOWN or SELECT to restart.",
                    "Premi SU, GIÙ o SELECT per riavviare."
                )
            ),
            restart_hint(locale),
        ),
    };

    draw_text_centered(display, title, CONTENT_LEFT, CONTENT_WIDTH, 236, large)?;
    draw_paragraph(
        display,
        &text,
        CONTENT_LEFT,
        290,
        body,
        CONTENT_WIDTH,
        14,
        6,
    )?;
    draw_footer(display, state, &hint)
}

/// Any key but BOOT restarts the device once the disk is, or failed to be,
/// connected.
fn restart_hint(locale: crate::regional::Locale) -> String {
    format!(
        "{} {}",
        t(locale, "UP/DOWN/SELECT", "SU/GI\u{00D9}/SELECT"),
        t(locale, "RESTART", "RIAVVIA")
    )
}
