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
        // Disk mode starts as this screen opens: this is only ever on the
        // glass for the moment that takes.
        UsbDiskPhase::Idle => (
            t(locale, "Connecting", "Collegamento"),
            t(
                locale,
                "Handing the microSD to the computer...",
                "Passaggio della microSD al computer...",
            )
            .to_string(),
            String::new(),
        ),
        UsbDiskPhase::Active => (
            t(locale, "Connected", "Collegato"),
            t(
                locale,
                "Connect the device to a computer with its USB cable: the microSD shows up as a disk, to copy books into BOOKS and audiobooks into AUDIO. The device cannot be used meanwhile. When done, eject the disk on the computer, then press any key: the device restarts and reads the new files.",
                "Collega il dispositivo al computer col cavo USB: la microSD compare come un disco, per copiare i libri in BOOKS e gli audiolibri in AUDIO. Nel frattempo il dispositivo non si usa. Finito, espelli il disco dal computer e premi un tasto qualsiasi: il dispositivo si riavvia e legge i nuovi file.",
            )
            .to_string(),
            restart_hint(locale),
        ),
        UsbDiskPhase::NoCard => (
            t(locale, "Card not readable", "Scheda non leggibile"),
            t(
                locale,
                "The microSD cannot be read, so there is nothing to show to the computer. Check that it is pushed all the way in, then restart the device.",
                "La microSD non si legge, quindi non c'\u{00E8} niente da mostrare al computer. Controlla che sia inserita fino in fondo, poi riavvia il dispositivo.",
            )
            .to_string(),
            footer_hints(locale, &[(FooterKey::Boot, back_action(locale))]),
        ),
        UsbDiskPhase::Failed(error) => (
            t(locale, "Not connected", "Collegamento non riuscito"),
            format!(
                "{} {error}",
                t(
                    locale,
                    "Press any key to restart.",
                    "Premi un tasto qualsiasi per riavviare."
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

/// Once the card is with the computer, or failed to get there, any key
/// restarts the device: BOOT too.
fn restart_hint(locale: crate::regional::Locale) -> String {
    t(locale, "ANY KEY RESTART", "UN TASTO RIAVVIA").to_string()
}
