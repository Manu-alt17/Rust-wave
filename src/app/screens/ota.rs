//! Minimal binary-prompt screen for GitHub-release OTA updates, and for the
//! bootloader update that can follow one.

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    build_info::FIRMWARE_VERSION,
    orientation::OrientedFrameBuffer,
    ota::{OtaCheckState, UpdateChannel},
    regional::Locale,
};

const LEFT: i32 = 22;
const TEXT_WIDTH: i32 = 480 - 2 * LEFT;
const STATUS_TOP: i32 = 234;
const LINE_STEP: i32 = 34;

pub fn render_ota_update(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let heading = state.display.heading_style();
    let body = state.display.body_style();
    let detail = state.display.detail_style();

    // "Update" reads the same in English and Italian chrome, so this header
    // does not need a locale-branched `t()` call.
    draw_header(display, state, "UPDATE")?;

    line(
        display,
        118,
        t(locale, "Installed", "Installata"),
        FIRMWARE_VERSION,
        body,
    )?;
    line(
        display,
        152,
        t(locale, "Channel", "Canale"),
        channel_label(locale, state.ota_channel),
        body,
    )?;
    line(
        display,
        186,
        "Bootloader",
        state
            .installed_bootloader
            .as_deref()
            .unwrap_or_else(|| t(locale, "unknown", "sconosciuto")),
        body,
    )?;

    let (status_heading, lines, footer_hint): (&str, [&str; 3], &str) = match &state.ota {
        OtaCheckState::Idle => (
            t(locale, "No check yet", "Nessun controllo eseguito"),
            [
                t(
                    locale,
                    "Press SELECT to check GitHub for a",
                    "Premi SELECT per controllare su GitHub",
                ),
                t(locale, "newer release.", "una versione più recente."),
                "",
            ],
            t(
                locale,
                "SELECT CHECK  UP/DOWN CHANNEL",
                "SELECT CONTROLLA  SU/GIÙ CANALE",
            ),
        ),
        OtaCheckState::Checking => (
            t(
                locale,
                "Checking for updates...",
                "Controllo aggiornamenti...",
            ),
            [
                t(
                    locale,
                    "Contacting GitHub, please wait.",
                    "Connessione a GitHub in corso.",
                ),
                "",
                "",
            ],
            "",
        ),
        OtaCheckState::UpToDate => (
            t(locale, "Up to date", "Aggiornato"),
            [
                t(
                    locale,
                    "You already have the latest release.",
                    "Hai già la versione più recente.",
                ),
                "",
                "",
            ],
            t(
                locale,
                "SELECT CHECK  UP/DOWN CHANNEL",
                "SELECT CONTROLLA  SU/GIÙ CANALE",
            ),
        ),
        OtaCheckState::UpdateAvailable { version, .. } => {
            return render_update_available(display, state, version, heading, body, detail);
        }
        OtaCheckState::CheckFailed(error) => (
            t(locale, "Check failed", "Controllo non riuscito"),
            [
                error.as_str(),
                "",
                t(
                    locale,
                    "Press SELECT to try again.",
                    "Premi SELECT per riprovare.",
                ),
            ],
            t(
                locale,
                "SELECT RETRY  UP/DOWN CHANNEL",
                "SELECT RIPROVA  SU/GIÙ CANALE",
            ),
        ),
        OtaCheckState::Installing => (
            t(
                locale,
                "Installing update...",
                "Installazione aggiornamento...",
            ),
            [
                t(
                    locale,
                    "Downloading and flashing.",
                    "Scaricamento e scrittura in corso.",
                ),
                t(
                    locale,
                    "Do not power off the device.",
                    "Non spegnere il dispositivo.",
                ),
                t(
                    locale,
                    "The device restarts automatically.",
                    "Il dispositivo si riavvia automaticamente.",
                ),
            ],
            t(locale, "PLEASE WAIT", "ATTENDERE PREGO"),
        ),
        OtaCheckState::InstallFailed(error) => (
            t(locale, "Install failed", "Installazione non riuscita"),
            [
                error.as_str(),
                t(
                    locale,
                    "The previous firmware is unaffected.",
                    "Il firmware precedente non è stato modificato.",
                ),
                t(
                    locale,
                    "Press SELECT to try again.",
                    "Premi SELECT per riprovare.",
                ),
            ],
            t(
                locale,
                "SELECT RETRY  UP/DOWN CHANNEL",
                "SELECT RIPROVA  SU/GIÙ CANALE",
            ),
        ),
        bootloader_state => return render_bootloader(display, state, bootloader_state),
    };

    Text::new(status_heading, Point::new(LEFT, STATUS_TOP), heading).draw(display)?;
    for (index, text) in lines.iter().enumerate() {
        if !text.is_empty() {
            Text::new(
                text,
                Point::new(LEFT, STATUS_TOP + 40 + index as i32 * LINE_STEP),
                body,
            )
            .draw(display)?;
        }
    }

    draw_footer(display, state, footer_hint)?;
    Ok(())
}

fn render_update_available(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    version: &str,
    heading: UiTextStyle,
    body: UiTextStyle,
    detail: UiTextStyle,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    Text::new(
        t(locale, "Update available", "Aggiornamento disponibile"),
        Point::new(LEFT, STATUS_TOP),
        heading,
    )
    .draw(display)?;
    line(
        display,
        STATUS_TOP + 40,
        t(locale, "New version", "Nuova versione"),
        version,
        body,
    )?;
    Text::new(
        t(
            locale,
            "Press SELECT to download and install.",
            "Premi SELECT per scaricare e installare.",
        ),
        Point::new(LEFT, STATUS_TOP + 80),
        body,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "The device restarts automatically once",
            "Il dispositivo si riavvia automaticamente",
        ),
        Point::new(LEFT, STATUS_TOP + 120),
        detail,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "the new firmware is written to flash.",
            "una volta scritto il nuovo firmware.",
        ),
        Point::new(LEFT, STATUS_TOP + 150),
        detail,
    )
    .draw(display)?;
    draw_footer(
        display,
        state,
        t(
            locale,
            "SELECT INSTALL  UP/DOWN CHANNEL",
            "SELECT INSTALLA  SU/GIÙ CANALE",
        ),
    )?;
    Ok(())
}

/// The bootloader states: a heading, an optional label/value line, a
/// paragraph wrapped to the screen, a footer.
fn render_bootloader(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    ota: &OtaCheckState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let unknown = t(locale, "unknown", "sconosciuto");
    let (heading, value, paragraph, footer): (&str, Option<(&str, &str)>, String, &str) =
        match ota {
            OtaCheckState::BootloaderAvailable { release, .. } => (
                t(locale, "Bootloader update", "Aggiornamento bootloader"),
                Some((t(locale, "In release", "Nella release"), release.as_str())),
                t(
                    locale,
                    "The release carries a different bootloader. SELECT downloads and checks it; nothing is written yet.",
                    "La release contiene un bootloader diverso. SELECT lo scarica e lo verifica, senza ancora scriverlo.",
                )
                .into(),
                t(
                    locale,
                    "SELECT DOWNLOAD  UP/DOWN CHANNEL",
                    "SELECT SCARICA  SU/GIÙ CANALE",
                ),
            ),
            OtaCheckState::PreparingBootloader => (
                t(locale, "Checking bootloader...", "Verifica bootloader..."),
                None,
                t(
                    locale,
                    "Downloading it and checking its signature.",
                    "Scaricamento e controllo in corso.",
                )
                .into(),
                "",
            ),
            OtaCheckState::BootloaderReady { new, .. } => (
                t(locale, "Bootloader checked", "Bootloader verificato"),
                Some((t(locale, "New", "Nuovo"), new.as_deref().unwrap_or(unknown))),
                t(
                    locale,
                    "Writing it takes under a second. Do not switch off meanwhile: a cut then can only be fixed over USB. Needs the battery at 50% or the USB cable.",
                    "La scrittura dura meno di un secondo. Non spegnere in quel momento: un'interruzione si ripara solo via USB. Serve la batteria al 50% o il cavo USB.",
                )
                .into(),
                t(locale, "SELECT WRITE", "SELECT SCRIVI"),
            ),
            OtaCheckState::InstallingBootloader => (
                t(locale, "Writing bootloader...", "Scrittura bootloader..."),
                None,
                t(
                    locale,
                    "Do not power off the device. It restarts when done.",
                    "Non spegnere il dispositivo. Al termine si riavvia.",
                )
                .into(),
                t(locale, "PLEASE WAIT", "ATTENDERE PREGO"),
            ),
            OtaCheckState::BootloaderInstalled => (
                t(locale, "Bootloader updated", "Bootloader aggiornato"),
                None,
                t(locale, "Restarting...", "Riavvio in corso...").into(),
                "",
            ),
            OtaCheckState::BootloaderDamaged(_) => (
                t(locale, "Bootloader damaged", "Bootloader danneggiato"),
                None,
                t(
                    locale,
                    "The new bootloader did not read back intact. Do not switch the device off: it may not start again. SELECT writes it once more; if it still fails, connect the device to a computer over USB and reflash it.",
                    "Il nuovo bootloader non si rilegge integro. Non spegnere il dispositivo: potrebbe non ripartire. SELECT lo riscrive; se non basta, collegalo a un computer via USB e riflashalo.",
                )
                .into(),
                t(locale, "SELECT WRITE AGAIN", "SELECT RISCRIVI"),
            ),
            OtaCheckState::BootloaderFailed(error) => (
                t(locale, "Bootloader not updated", "Bootloader non aggiornato"),
                None,
                error.clone(),
                t(
                    locale,
                    "SELECT CHECK  UP/DOWN CHANNEL",
                    "SELECT CONTROLLA  SU/GIÙ CANALE",
                ),
            ),
            _ => (unknown, None, String::new(), ""),
        };

    Text::new(
        heading,
        Point::new(LEFT, STATUS_TOP),
        state.display.heading_style(),
    )
    .draw(display)?;
    let mut y = STATUS_TOP + 40;
    if let Some((label, value)) = value {
        line(display, y, label, value, body)?;
        y += 40;
    }
    for text in wrap(body, &paragraph, TEXT_WIDTH) {
        Text::new(&text, Point::new(LEFT, y), body).draw(display)?;
        y += LINE_STEP;
    }
    draw_footer(display, state, footer)?;
    Ok(())
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

fn channel_label(locale: Locale, channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => t(locale, "Stable", "Stabile"),
        UpdateChannel::Beta => "Beta",
    }
}

fn line(
    display: &mut OrientedFrameBuffer<'_>,
    y: i32,
    label: &str,
    value: &str,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    Text::new(label, Point::new(LEFT, y), style).draw(display)?;
    Text::new(value, Point::new(194, y), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_ota_update;
    use crate::{
        app::AppState,
        bootloader_update::BootloaderAsset,
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        ota::{OtaCheckState, UpdateChannel},
    };

    #[test]
    fn renders_every_ota_state_without_panicking() {
        let asset = BootloaderAsset {
            download_url: "https://example.com/x-bootloader.img".into(),
            sha256: [0; 32],
            size: 19_008,
        };
        let states = [
            OtaCheckState::Idle,
            OtaCheckState::Checking,
            OtaCheckState::UpToDate,
            OtaCheckState::UpdateAvailable {
                version: "v1.3.0".into(),
                download_url: "https://example.com/x.bin".into(),
            },
            OtaCheckState::CheckFailed("network error".into()),
            OtaCheckState::Installing,
            OtaCheckState::InstallFailed("flash write failed".into()),
            OtaCheckState::BootloaderAvailable {
                release: "v1.5.0-beta.2".into(),
                installed: Some("v5.5.1, 2026-10-02".into()),
                asset,
            },
            OtaCheckState::PreparingBootloader,
            OtaCheckState::BootloaderReady {
                release: "v1.5.0-beta.2".into(),
                new: None,
            },
            OtaCheckState::InstallingBootloader,
            OtaCheckState::BootloaderInstalled,
            OtaCheckState::BootloaderFailed(
                "Batteria al 35%: caricala almeno al 50% o collega il cavo USB.".into(),
            ),
            OtaCheckState::BootloaderDamaged("readback".into()),
        ];
        for state_value in states {
            for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
                let mut frame = FrameBuffer::new_white();
                let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
                let mut state = AppState::default();
                state.ota = state_value.clone();
                state.ota_channel = channel;
                render_ota_update(&mut display, &state).unwrap();
            }
        }
    }
}
