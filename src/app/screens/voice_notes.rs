//! SD-backed voice notes list, details, title editor and recording screens.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Drawable, Point, Primitive, Size},
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    app::{
        i18n::t,
        state::AppState,
        typography::{Text, UiTextStyle},
        widgets::{footer::draw_footer, header::draw_header},
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
    voice_note_metadata::format_storage_bytes,
    voice_notes::{format_duration, VoiceNotesMode, VOICE_TITLE_EDITOR_KEY_ROWS},
};

pub fn render_voice_notes(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let heading = state.display.heading_style();
    let voice = &state.voice_notes;
    let visible = voice.visible_note_range();

    draw_header(display, state, t(locale, "VOICE NOTES", "NOTE VOCALI"))?;
    let recordings = if voice.notes.is_empty() {
        t(locale, "Recordings", "Registrazioni").to_string()
    } else {
        match locale {
            Locale::English => format!(
                "Recordings {}-{} of {}",
                visible.start + 1,
                visible.end,
                voice.notes.len()
            ),
            Locale::Italian => format!(
                "Registrazioni {}-{} di {}",
                visible.start + 1,
                visible.end,
                voice.notes.len()
            ),
        }
    };
    Text::new(&recordings, Point::new(22, 112), heading).draw(display)?;
    draw_action(
        display,
        152,
        t(locale, "Record new note", "Registra nuova nota"),
        voice.selected == 0,
        body,
    )?;
    let gain = match locale {
        Locale::English => format!(
            "Microphone gain: {} ({})",
            voice.mic_gain.label(),
            voice.mic_gain.db_label()
        ),
        Locale::Italian => format!(
            "Guadagno microfono: {} ({})",
            voice.mic_gain.label_i18n(locale),
            voice.mic_gain.db_label()
        ),
    };
    draw_action(display, 208, &gain, voice.selected == 1, body)?;
    for (visible_index, note_index) in visible.enumerate() {
        let note = &voice.notes[note_index];
        let row = note_index + 2;
        let duration = format_duration(note.duration_seconds);
        let label = format!("{}   {}", note.title, duration);
        draw_action(
            display,
            262 + visible_index as i32 * 54,
            &label,
            voice.selected == row,
            body,
        )?;
    }
    if let Some(error) = voice.error.as_deref() {
        Text::new(error, Point::new(22, 640), state.display.detail_style()).draw(display)?;
    }
    Ok(())
}

pub fn render_voice_note_details(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let metadata = state.display.body_style();
    if state.voice_notes.title_editing {
        return render_voice_note_title_editor(display, state);
    }
    draw_header(display, state, t(locale, "VOICE NOTE", "NOTA VOCALE"))?;
    let Some(note) = state.voice_notes.selected_note() else {
        Text::new(
            t(
                locale,
                "No saved note selected.",
                "Nessuna nota salvata selezionata.",
            ),
            Point::new(22, 186),
            detail,
        )
        .draw(display)?;
        return Ok(());
    };
    if state.voice_notes.delete_confirmation {
        return render_voice_note_delete_confirmation(display, state);
    }

    Text::new(
        &note.title,
        Point::new(22, 112),
        state.display.heading_style(),
    )
    .draw(display)?;
    line(
        display,
        158,
        t(locale, "File", "File"),
        &note.file_name,
        metadata,
    )?;
    line(
        display,
        194,
        t(locale, "Recorded", "Registrata"),
        &note.recorded_at,
        metadata,
    )?;
    line(
        display,
        230,
        t(locale, "Duration", "Durata"),
        &format_duration(note.duration_seconds),
        metadata,
    )?;
    line(
        display,
        266,
        t(locale, "Available", "Disponibile"),
        &format_storage_bytes(state.voice_notes.available_storage_bytes),
        metadata,
    )?;
    line(
        display,
        302,
        t(locale, "Playback", "Riproduzione"),
        &format!(
            "{} / {}",
            format_duration(state.voice_notes.playback_elapsed_seconds()),
            format_duration(note.duration_seconds)
        ),
        metadata,
    )?;
    draw_action(
        display,
        340,
        if state.voice_notes.is_playing_selected() {
            t(locale, "Stop playback", "Interrompi riproduzione")
        } else {
            t(locale, "Play note", "Riproduci nota")
        },
        state.voice_notes.detail_selected == 0,
        body,
    )?;
    draw_action(
        display,
        390,
        t(locale, "Edit friendly title", "Modifica titolo"),
        state.voice_notes.detail_selected == 1,
        body,
    )?;
    draw_action(
        display,
        440,
        t(locale, "Export / download", "Esporta / scarica"),
        state.voice_notes.detail_selected == 2,
        body,
    )?;
    draw_action(
        display,
        490,
        t(locale, "Delete note", "Elimina nota"),
        state.voice_notes.detail_selected == 3,
        body,
    )?;
    draw_action(
        display,
        540,
        t(locale, "Return to Voice Notes", "Torna a Note vocali"),
        state.voice_notes.detail_selected == 4,
        body,
    )?;
    if state.voice_notes.export_file.as_deref() == Some(note.file_name.as_str()) {
        line(
            display,
            604,
            t(locale, "LAN", "LAN"),
            state.wifi_transfer.url_label(),
            metadata,
        )?;
        line(
            display,
            636,
            t(locale, "Code", "Codice"),
            state.wifi_transfer.code_label(),
            metadata,
        )?;
        line(
            display,
            668,
            t(locale, "Path", "Percorso"),
            &format!("VOICE/{}", note.file_name),
            metadata,
        )?;
    } else if let Some(error) = state.voice_notes.error.as_deref() {
        Text::new(error, Point::new(22, 636), detail).draw(display)?;
    }
    draw_footer(display, state, t(locale, "SELECT RUN", "SELECT ESEGUI"))?;
    Ok(())
}

fn render_voice_note_title_editor(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let voice = &state.voice_notes;
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    let title = voice.title_edit_buffer.iter().collect::<String>();
    draw_header(
        display,
        state,
        t(locale, "VOICE NOTE TITLE", "TITOLO NOTA VOCALE"),
    )?;
    Text::new(
        t(locale, "Friendly title", "Titolo descrittivo"),
        Point::new(22, 124),
        body,
    )
    .draw(display)?;
    Rectangle::new(Point::new(22, 138), Size::new(436, 54))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(display)?;
    let title_label = if title.is_empty() {
        "_"
    } else {
        title.as_str()
    };
    Text::new(title_label, Point::new(34, 172), body).draw(display)?;
    Text::new(
        t(
            locale,
            "Internal WAV filename remains unchanged.",
            "Il nome del file WAV interno resta invariato.",
        ),
        Point::new(22, 230),
        detail,
    )
    .draw(display)?;
    draw_voice_title_keyboard(display, state)?;
    draw_footer(
        display,
        state,
        t(
            locale,
            "HOLD H/V  SELECT KEY",
            "TIENI PREMUTO O/V  SELECT TASTO",
        ),
    )?;
    Ok(())
}

fn draw_voice_title_keyboard(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let voice = &state.voice_notes;
    for (row_index, row) in VOICE_TITLE_EDITOR_KEY_ROWS.iter().enumerate() {
        for (column_index, label) in row.iter().enumerate() {
            let index = row_index * 7 + column_index;
            let left = 22 + column_index as i32 * 62;
            let top = 268 + row_index as i32 * 54;
            let selected = voice.title_editor_selected_key_index() == index;
            Rectangle::new(Point::new(left, top), Size::new(58, 46))
                .into_styled(PrimitiveStyle::with_stroke(
                    BinaryColor::On,
                    if selected { 3 } else { 1 },
                ))
                .draw(display)?;
            Text::new(
                label,
                Point::new(left + if label.len() > 3 { 4 } else { 15 }, top + 29),
                state.display.detail_style(),
            )
            .draw(display)?;
        }
    }
    Ok(())
}

fn render_voice_note_delete_confirmation(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let Some(note) = state.voice_notes.selected_note() else {
        return Ok(());
    };
    let body = state.display.body_style();
    let detail = state.display.detail_style();
    Text::new(
        t(locale, "DELETE VOICE NOTE?", "ELIMINARE LA NOTA VOCALE?"),
        Point::new(22, 152),
        state.display.heading_style(),
    )
    .draw(display)?;
    Text::new(&note.title, Point::new(22, 218), body).draw(display)?;
    Text::new(&note.file_name, Point::new(22, 264), detail).draw(display)?;
    Text::new(
        t(
            locale,
            "This permanently removes the WAV file.",
            "Questa azione rimuove definitivamente il file WAV.",
        ),
        Point::new(22, 324),
        detail,
    )
    .draw(display)?;
    draw_action(
        display,
        402,
        t(locale, "Cancel", "Annulla"),
        state.voice_notes.delete_confirm_selected == 0,
        body,
    )?;
    draw_action(
        display,
        470,
        t(locale, "Delete permanently", "Elimina definitivamente"),
        state.voice_notes.delete_confirm_selected == 1,
        body,
    )?;
    draw_footer(
        display,
        state,
        t(locale, "SELECT CONFIRM", "SELECT CONFERMA"),
    )?;
    Ok(())
}

pub fn render_voice_note_recording(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let body = state.display.body_style();
    let heading = state.display.heading_style();
    let detail = state.display.detail_style();
    let voice = &state.voice_notes;
    let elapsed = format_duration(voice.elapsed_seconds);
    let file = voice.active_file.as_deref().unwrap_or("VOICE---.WAV");
    draw_header(
        display,
        state,
        t(locale, "RECORD VOICE NOTE", "REGISTRA NOTA VOCALE"),
    )?;
    Text::new(file, Point::new(22, 130), heading).draw(display)?;
    line(
        display,
        176,
        t(locale, "Elapsed", "Trascorso"),
        &elapsed,
        body,
    )?;
    line(
        display,
        224,
        t(locale, "PCM bytes", "Byte PCM"),
        &voice.pcm_bytes.to_string(),
        body,
    )?;
    line(
        display,
        272,
        t(locale, "Peak", "Picco"),
        &voice.peak.to_string(),
        body,
    )?;
    line(
        display,
        320,
        t(locale, "Started", "Iniziata"),
        voice.active_recorded_at.as_deref().unwrap_or(t(
            locale,
            "DATE UNKNOWN",
            "DATA SCONOSCIUTA",
        )),
        detail,
    )?;
    line(
        display,
        368,
        t(locale, "Mic gain", "Guadagno mic"),
        &format!(
            "{} ({})",
            voice.mic_gain.label_i18n(locale),
            voice.mic_gain.db_label()
        ),
        body,
    )?;
    line(
        display,
        416,
        t(locale, "Clipped", "Troncati"),
        &voice.clipped_samples.to_string(),
        body,
    )?;
    match voice.mode {
        VoiceNotesMode::Recording => {
            Text::new(
                t(
                    locale,
                    "Streaming VOICE###.TMP",
                    "Streaming su VOICE###.TMP",
                ),
                Point::new(22, 500),
                body,
            )
            .draw(display)?;
            Text::new(
                t(
                    locale,
                    "UP / DOWN pause. SELECT stop + save.",
                    "SU / GIU pausa. SELECT ferma e salva.",
                ),
                Point::new(22, 552),
                detail,
            )
            .draw(display)?;
        }
        VoiceNotesMode::Paused => {
            Text::new(
                t(
                    locale,
                    "Recording paused. WAV remains open.",
                    "Registrazione in pausa. Il file WAV resta aperto.",
                ),
                Point::new(22, 500),
                body,
            )
            .draw(display)?;
            Text::new(
                t(
                    locale,
                    "UP / DOWN resume. SELECT stop + save.",
                    "SU / GIU riprendi. SELECT ferma e salva.",
                ),
                Point::new(22, 552),
                detail,
            )
            .draw(display)?;
        }
        VoiceNotesMode::Playing => {
            Text::new(
                t(
                    locale,
                    "Saved WAV playback active.",
                    "Riproduzione del WAV salvato attiva.",
                ),
                Point::new(22, 500),
                body,
            )
            .draw(display)?;
            Text::new(
                t(
                    locale,
                    "Press BOOT to stop and return.",
                    "Premi BOOT per fermare e tornare indietro.",
                ),
                Point::new(22, 552),
                body,
            )
            .draw(display)?;
        }
        VoiceNotesMode::Saved => {
            Text::new(
                t(
                    locale,
                    "Saved as recovery-safe WAV.",
                    "Salvata in un WAV a prova di interruzioni.",
                ),
                Point::new(22, 500),
                body,
            )
            .draw(display)?;
            Text::new(
                t(
                    locale,
                    "Press BOOT to return.",
                    "Premi BOOT per tornare indietro.",
                ),
                Point::new(22, 552),
                body,
            )
            .draw(display)?;
        }
        VoiceNotesMode::Error => {
            Text::new(
                voice.error.as_deref().unwrap_or(t(
                    locale,
                    "Recording failed",
                    "Registrazione non riuscita",
                )),
                Point::new(22, 500),
                detail,
            )
            .draw(display)?;
            Text::new(
                t(
                    locale,
                    "Press BOOT to return.",
                    "Premi BOOT per tornare indietro.",
                ),
                Point::new(22, 552),
                body,
            )
            .draw(display)?;
        }
        VoiceNotesMode::Idle => {
            Text::new(
                t(
                    locale,
                    "Preparing microphone capture...",
                    "Preparazione acquisizione microfono...",
                ),
                Point::new(22, 500),
                body,
            )
            .draw(display)?;
        }
    }
    draw_footer(
        display,
        state,
        t(
            locale,
            "UP DOWN PAUSE / RESUME  SELECT STOP + SAVE",
            "SU GIU PAUSA / RIPRENDI  SELECT FERMA + SALVA",
        ),
    )?;
    Ok(())
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
    Rectangle::new(Point::new(22, top), Size::new(436, 46))
        .into_styled(if selected {
            PrimitiveStyle::with_stroke(BinaryColor::On, 5)
        } else {
            PrimitiveStyle::with_stroke(BinaryColor::On, 1)
        })
        .draw(display)?;
    Text::new(
        if selected { ">" } else { " " },
        Point::new(38, top + 30),
        style,
    )
    .draw(display)?;
    Text::new(label, Point::new(68, top + 30), style).draw(display)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_voice_note_details, render_voice_note_recording, render_voice_notes};
    use crate::{
        app::AppState, framebuffer::FrameBuffer, orientation::OrientedFrameBuffer,
        voice_notes::VoiceNoteEntry,
    };

    #[test]
    fn voice_note_screens_render_without_sd_card() {
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let state = AppState::default();
        render_voice_notes(&mut display, &state).unwrap();
        render_voice_note_details(&mut display, &state).unwrap();
        render_voice_note_recording(&mut display, &state).unwrap();
    }

    #[test]
    fn friendly_title_grid_editor_renders_with_saved_note() {
        let mut state = AppState::default();
        state.voice_notes.notes.push(VoiceNoteEntry {
            file_name: "VOICE001.WAV".into(),
            title: "VOICE NOTE 001".into(),
            recorded_at: "2026-06-06  11:43:24".into(),
            wav_bytes: 44,
            pcm_bytes: 0,
            duration_seconds: 0,
        });
        state.voice_notes.selected = 2;
        state.voice_notes.begin_title_edit();
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        render_voice_note_details(&mut display, &state).unwrap();
    }
}
