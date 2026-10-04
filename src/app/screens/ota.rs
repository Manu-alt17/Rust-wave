//! Software Update: firmware updates from GitHub releases, and the
//! bootloader update that can follow one.
//!
//! Top to bottom: what is installed, what the last check found (a heading
//! and a wrapped paragraph), then two rows -- the action the current state
//! offers and the release channel. The rocker moves between the two rows
//! and SELECT runs the selected one, so the channel never changes by
//! brushing the rocker.

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::{
        i18n::t,
        state::AppState,
        widgets::{
            footer::{draw_footer, select_and_back},
            header::draw_header,
            layout::{CONTENT_BOTTOM, CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE},
            list::{draw_field, draw_list_row, draw_section_title, ROW_GAP, ROW_STEP},
            progress::draw_progress_bar,
            text::{draw_paragraph, draw_text_fit},
        },
    },
    build_info::FIRMWARE_VERSION,
    orientation::OrientedFrameBuffer,
    ota::{InstallProgress, OtaCheckState, UpdateChannel},
    regional::Locale,
};

/// Top of the action row; the channel row follows it. Anchored to the
/// bottom so the rows stay put while the text above changes length.
const ROWS_TOP: i32 = CONTENT_BOTTOM - 2 * ROW_STEP + ROW_GAP;

pub fn render_ota_update(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    let body = preferences.body_style();
    let unknown = t(locale, "unknown", "sconosciuto");

    draw_header(display, state, t(locale, "UPDATE", "AGGIORNA"))?;

    let mut baseline = FIRST_BASELINE;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Installed", "Installata"),
        FIRMWARE_VERSION,
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        t(locale, "Channel", "Canale"),
        channel_label(locale, state.ota_channel),
    )?;
    baseline = draw_field(
        display,
        preferences,
        baseline,
        "Bootloader",
        state.installed_bootloader.as_deref().unwrap_or(unknown),
    )?;

    let status = status_text(state);
    baseline = draw_section_title(display, preferences, baseline + 18, status.heading)?;
    if let Some((label, value)) = status.field {
        baseline = draw_field(display, preferences, baseline, label, value)?;
    }
    baseline = draw_paragraph(
        display,
        &status.paragraph,
        CONTENT_LEFT,
        baseline,
        body,
        CONTENT_WIDTH,
        8,
        6,
    )?;
    if let (OtaCheckState::Installing, Some(progress)) = (&state.ota, state.ota_install_progress) {
        draw_install_progress(display, state, baseline + 14, progress)?;
    }

    let Some(action) = action_label(state) else {
        // A check or an install is under way: nothing to select.
        return draw_footer(display, state, t(locale, "PLEASE WAIT", "ATTENDERE"));
    };
    draw_list_row(
        display,
        preferences,
        ROWS_TOP,
        &action,
        "",
        state.ota_action_selected == 0,
    )?;
    draw_list_row(
        display,
        preferences,
        ROWS_TOP + ROW_STEP,
        t(locale, "Channel", "Canale"),
        channel_label(locale, state.ota_channel),
        state.ota_action_selected == 1,
    )?;
    let hint = if state.ota_action_selected == 1 {
        t(locale, "CHANGE", "CAMBIA")
    } else {
        t(locale, "RUN", "ESEGUI")
    };
    draw_footer(display, state, &select_and_back(locale, hint))
}

/// Height of the download bar.
const PROGRESS_BAR_HEIGHT: i32 = 16;

/// The download so far: a bar when the size of the image is known, and
/// under it how much arrived.
fn draw_install_progress(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    top: i32,
    progress: InstallProgress,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let mark = match locale {
        Locale::English => '.',
        Locale::Italian => ',',
    };
    let received = InstallProgress::megabytes_label(progress.received_bytes, mark);
    let downloaded = t(locale, "Downloaded", "Scaricati");
    let (label, text_top) = match (progress.percent(), progress.total_bytes) {
        (Some(percent), Some(total)) => {
            draw_progress_bar(
                display,
                CONTENT_LEFT,
                top,
                CONTENT_WIDTH,
                PROGRESS_BAR_HEIGHT,
                percent,
            )?;
            (
                format!(
                    "{percent}% \u{00B7} {received} {} {}",
                    t(locale, "of", "di"),
                    InstallProgress::megabytes_label(total, mark)
                ),
                top + PROGRESS_BAR_HEIGHT + 12,
            )
        }
        _ => (format!("{downloaded} {received}"), top),
    };
    draw_text_fit(
        display,
        &label,
        Point::new(CONTENT_LEFT, text_top + i32::from(body.line_height())),
        body,
        CONTENT_WIDTH,
    )
}

/// What the screen says about the current state.
struct StatusText<'a> {
    heading: &'static str,
    field: Option<(&'static str, &'a str)>,
    paragraph: String,
}

fn status_text(state: &AppState) -> StatusText<'_> {
    let locale = state.regional.locale;
    let unknown = t(locale, "unknown", "sconosciuto");
    let plain = |heading: &'static str, paragraph: &str| StatusText {
        heading,
        field: None,
        paragraph: paragraph.to_string(),
    };
    match &state.ota {
        OtaCheckState::Idle => plain(
            t(locale, "No check yet", "Nessun controllo eseguito"),
            t(
                locale,
                "Check GitHub for a newer release.",
                "Controlla se su GitHub c'\u{00E8} una versione pi\u{00F9} recente.",
            ),
        ),
        OtaCheckState::Checking => plain(
            t(
                locale,
                "Checking for updates...",
                "Controllo aggiornamenti...",
            ),
            t(
                locale,
                "Contacting GitHub, please wait.",
                "Connessione a GitHub in corso.",
            ),
        ),
        OtaCheckState::UpToDate => plain(
            t(locale, "Up to date", "Aggiornato"),
            t(
                locale,
                "You already have the latest release.",
                "Hai gi\u{00E0} la versione pi\u{00F9} recente.",
            ),
        ),
        OtaCheckState::UpdateAvailable { version, .. } => StatusText {
            heading: t(locale, "Update available", "Aggiornamento disponibile"),
            field: Some((t(locale, "New version", "Nuova versione"), version.as_str())),
            paragraph: if state.ota_install_armed {
                t(
                    locale,
                    "Press SELECT again to install. Do not power off the device meanwhile.",
                    "Premi di nuovo SELECT per installare. Non spegnere il dispositivo nel frattempo.",
                )
            } else {
                t(
                    locale,
                    "Installing downloads the new firmware, writes it and restarts the device.",
                    "L'installazione scarica il nuovo firmware, lo scrive e riavvia il dispositivo.",
                )
            }
            .to_string(),
        },
        OtaCheckState::CheckFailed(error) => {
            plain(t(locale, "Check failed", "Controllo non riuscito"), error)
        }
        OtaCheckState::Installing => plain(
            t(
                locale,
                "Installing update...",
                "Installazione aggiornamento...",
            ),
            t(
                locale,
                "Downloading and writing. Do not power off the device: it restarts by itself when done.",
                "Scaricamento e scrittura in corso. Non spegnere il dispositivo: al termine si riavvia da solo.",
            ),
        ),
        OtaCheckState::InstallFailed(error) => StatusText {
            heading: t(locale, "Install failed", "Installazione non riuscita"),
            field: None,
            paragraph: format!(
                "{error}\n{}",
                t(
                    locale,
                    "The previous firmware is unaffected.",
                    "Il firmware precedente non \u{00E8} stato modificato.",
                )
            ),
        },
        OtaCheckState::BootloaderAvailable { release, .. } => StatusText {
            heading: t(locale, "Bootloader update", "Aggiornamento bootloader"),
            field: Some((t(locale, "In release", "Nella release"), release.as_str())),
            paragraph: t(
                locale,
                "The release carries a different bootloader. Downloading checks it; nothing is written yet.",
                "La release contiene un bootloader diverso. Lo scaricamento lo verifica, senza ancora scriverlo.",
            )
            .to_string(),
        },
        OtaCheckState::PreparingBootloader => plain(
            t(locale, "Checking bootloader...", "Verifica bootloader..."),
            t(
                locale,
                "Downloading it and checking its signature.",
                "Scaricamento e controllo in corso.",
            ),
        ),
        OtaCheckState::BootloaderReady { new, .. } => StatusText {
            heading: t(locale, "Bootloader checked", "Bootloader verificato"),
            field: Some((t(locale, "New", "Nuovo"), new.as_deref().unwrap_or(unknown))),
            paragraph: t(
                locale,
                "Writing it takes under a second. Do not switch off meanwhile: a cut then can only be fixed over USB. Needs the battery at 50% or the USB cable.",
                "La scrittura dura meno di un secondo. Non spegnere in quel momento: un'interruzione si ripara solo via USB. Serve la batteria al 50% o il cavo USB.",
            )
            .to_string(),
        },
        OtaCheckState::InstallingBootloader => plain(
            t(locale, "Writing bootloader...", "Scrittura bootloader..."),
            t(
                locale,
                "Do not power off the device. It restarts when done.",
                "Non spegnere il dispositivo. Al termine si riavvia.",
            ),
        ),
        OtaCheckState::BootloaderInstalled => plain(
            t(locale, "Bootloader updated", "Bootloader aggiornato"),
            t(locale, "Restarting...", "Riavvio in corso..."),
        ),
        OtaCheckState::BootloaderDamaged(_) => plain(
            t(locale, "Bootloader damaged", "Bootloader danneggiato"),
            t(
                locale,
                "The new bootloader did not read back intact. Do not switch the device off: it may not start again. Write it once more; if it still fails, connect the device to a computer over USB and reflash it.",
                "Il nuovo bootloader non si rilegge integro. Non spegnere il dispositivo: potrebbe non ripartire. Riscrivilo; se non basta, collegalo a un computer via USB e riflashalo.",
            ),
        ),
        OtaCheckState::BootloaderFailed(error) => plain(
            t(
                locale,
                "Bootloader not updated",
                "Bootloader non aggiornato",
            ),
            error,
        ),
    }
}

/// Label of the action row for the current state, or `None` while a check
/// or an install is under way and there is nothing to run.
fn action_label(state: &AppState) -> Option<String> {
    let locale = state.regional.locale;
    if !state.ota.can_check() {
        return None;
    }
    Some(match &state.ota {
        OtaCheckState::UpdateAvailable { .. } if state.ota_install_armed => {
            t(locale, "Confirm: install now", "Conferma: installa ora").to_string()
        }
        OtaCheckState::UpdateAvailable { version, .. } => {
            format!("{} {version}", t(locale, "Install", "Installa"))
        }
        OtaCheckState::CheckFailed(_) | OtaCheckState::InstallFailed(_) => {
            t(locale, "Try again", "Riprova").to_string()
        }
        OtaCheckState::BootloaderAvailable { .. } => {
            t(locale, "Download the bootloader", "Scarica il bootloader").to_string()
        }
        OtaCheckState::BootloaderReady { .. } => {
            t(locale, "Write the bootloader", "Scrivi il bootloader").to_string()
        }
        OtaCheckState::BootloaderDamaged(_) => t(
            locale,
            "Write the bootloader again",
            "Riscrivi il bootloader",
        )
        .to_string(),
        _ => t(locale, "Check for updates", "Controlla aggiornamenti").to_string(),
    })
}

fn channel_label(locale: Locale, channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => t(locale, "Stable", "Stabile"),
        UpdateChannel::Beta => "Beta",
    }
}

#[cfg(test)]
mod tests {
    use super::render_ota_update;
    use crate::{
        app::AppState,
        bootloader_update::BootloaderAsset,
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        ota::{InstallProgress, OtaCheckState, UpdateChannel},
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
        for progress in [
            InstallProgress::new(300_000, None),
            InstallProgress::new(900_000, Some(1_900_000)),
        ] {
            let mut frame = FrameBuffer::new_white();
            let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
            let mut state = AppState::default();
            state.ota = OtaCheckState::Installing;
            state.ota_install_progress = Some(progress);
            render_ota_update(&mut display, &state).unwrap();
        }
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
