//! "Connect to PC": the microSD as a USB disk.

use core::convert::Infallible;

use embedded_graphics::{pixelcolor::BinaryColor, prelude::Point};
use embedded_iconoir::{icons::size96px::devices::Laptop, prelude::IconoirNewIcon};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header, home_tile::draw_iconoir_icon},
    },
    orientation::OrientedFrameBuffer,
    usb_disk::UsbDiskPhase,
};

const LEFT: i32 = 22;
const WIDTH: i32 = 436;

pub fn render_usb_disk(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let large = state.display.large_style();
    let body = state.display.body_style();
    draw_header(display, state, t(locale, "CONNECT TO PC", "COLLEGA AL PC"))?;
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
            t(locale, "SELECT CONNECT", "SELECT COLLEGA"),
        ),
        UsbDiskPhase::Active => (
            t(locale, "Connected", "Collegato"),
            t(
                locale,
                "The computer now sees the microSD as a disk. Copy the files, eject the disk on the computer, then press UP, DOWN or SELECT to restart.",
                "Il computer ora vede la microSD come un disco. Copia i file, espelli il disco dal computer, poi premi SU, GIÙ o SELECT per riavviare.",
            )
            .to_string(),
            t(locale, "UP/DOWN/SELECT RESTART", "SU/GIÙ/SELECT RIAVVIA"),
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
            t(locale, "UP/DOWN/SELECT RESTART", "SU/GIÙ/SELECT RIAVVIA"),
        ),
    };

    let title_width = large.text_width(title);
    Text::new(
        title,
        Point::new(LEFT + (WIDTH - title_width) / 2, 236),
        large,
    )
    .draw(display)?;
    let mut baseline = 290;
    for line in wrap(body, &text, WIDTH) {
        Text::new(&line, Point::new(LEFT, baseline), body).draw(display)?;
        baseline += i32::from(body.line_height()) + 6;
    }
    draw_footer(display, state, hint)
}

fn wrap(style: UiTextStyle, text: &str, max_width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && style.text_width(&candidate) > max_width {
            lines.push(std::mem::replace(&mut current, word.to_string()));
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}
