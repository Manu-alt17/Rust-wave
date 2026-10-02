//! Minimal binary-prompt screen for GitHub-release OTA updates.

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
    };

    Text::new(status_heading, Point::new(22, 200), heading).draw(display)?;
    for (index, text) in lines.iter().enumerate() {
        if !text.is_empty() {
            Text::new(text, Point::new(22, 240 + index as i32 * 34), body).draw(display)?;
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
        Point::new(22, 200),
        heading,
    )
    .draw(display)?;
    line(
        display,
        240,
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
        Point::new(22, 280),
        body,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "The device restarts automatically once",
            "Il dispositivo si riavvia automaticamente",
        ),
        Point::new(22, 320),
        detail,
    )
    .draw(display)?;
    Text::new(
        t(
            locale,
            "the new firmware is written to flash.",
            "una volta scritto il nuovo firmware.",
        ),
        Point::new(22, 350),
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
    Text::new(label, Point::new(22, y), style).draw(display)?;
    Text::new(value, Point::new(194, y), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::render_ota_update;
    use crate::{
        app::AppState,
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        ota::{OtaCheckState, UpdateChannel},
    };

    #[test]
    fn renders_every_ota_state_without_panicking() {
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
