//! The first-run pages and the "card not readable" warning.
//!
//! Every page is the same shape: a title, a few lines that say what the
//! page is for, then the rows to choose from, the first being what the page
//! proposes and the last moving on without it.

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::{Point, Size},
};
use embedded_iconoir::{icons::size48px::other::Language, prelude::IconoirNewIcon};

use crate::{
    app::{
        i18n::t,
        setup::{SetupPage, SETUP_NUMBERED_PAGES},
        state::AppState,
        widgets::{
            footer::{draw_footer, draw_footer_paged, footer_hints, select_and_back, FooterKey},
            header::draw_header,
            home_tile::{draw_icon_tile, TILE_GAP_X},
            layout::{CONTENT_LEFT, CONTENT_WIDTH, FIRST_BASELINE, FIRST_ROW_TOP},
            list::{
                draw_field, draw_list_row, draw_section_title, draw_tall_row, ROW_STEP,
                TALL_ROW_STEP,
            },
            text::draw_paragraph,
        },
    },
    orientation::OrientedFrameBuffer,
    regional::Locale,
};

use super::clock::long_date;

/// Gap between a page's text and its first row.
const ROWS_GAP: i32 = 6;
/// Height of the two language tiles.
const LANGUAGE_TILE_HEIGHT: u32 = 150;

/// The footer of a numbered page: what SELECT and BOOT do, and the page.
fn draw_page_footer(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    draw_footer_paged(
        display,
        state,
        &select_and_back(locale, t(locale, "CHOOSE", "SCEGLI")),
        state
            .setup
            .page
            .number()
            .map(|number| (number, SETUP_NUMBERED_PAGES)),
    )
}

/// A page made of a title, a paragraph, optional label/value fields and
/// the rows to choose from.
fn draw_text_page(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
    header: &str,
    title: &str,
    text: &str,
    fields: &[(&str, &str)],
    rows: &[&str],
) -> Result<(), Infallible> {
    let preferences = state.display;
    draw_header(display, state, header)?;
    let next = draw_section_title(display, preferences, FIRST_BASELINE, title)?;
    let mut baseline = draw_paragraph(
        display,
        text,
        CONTENT_LEFT,
        next,
        preferences.body_style(),
        CONTENT_WIDTH,
        8,
        6,
    )?;
    for (label, value) in fields {
        baseline = draw_field(display, preferences, baseline + 8, label, value)?;
    }
    let mut top = baseline + ROWS_GAP;
    for (index, row) in rows.iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            top,
            row,
            "",
            index == state.setup.selected,
        )?;
        top += ROW_STEP;
    }
    draw_page_footer(display, state)
}

/// The language, asked before anything else and so in both languages.
fn draw_language(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let preferences = state.display;
    draw_header(display, state, "RUSTMIX")?;
    let next = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        "Language \u{00B7} Lingua",
    )?;
    let after = draw_paragraph(
        display,
        "Choose the language of the menus.\nScegli la lingua dei menu.",
        CONTENT_LEFT,
        next,
        preferences.body_style(),
        CONTENT_WIDTH,
        4,
        6,
    )?;
    let tile = Size::new(
        ((CONTENT_WIDTH - TILE_GAP_X) / 2) as u32,
        LANGUAGE_TILE_HEIGHT,
    );
    let top = after + 10;
    for (index, locale) in [Locale::English, Locale::Italian].into_iter().enumerate() {
        draw_icon_tile(
            display,
            Point::new(
                CONTENT_LEFT + index as i32 * (tile.width as i32 + TILE_GAP_X),
                top,
            ),
            tile,
            locale.display_label(),
            &Language::new(BinaryColor::On),
            state.setup.selected == index,
            preferences,
        )?;
    }
    // On a new card there is nowhere to go back to yet.
    let hint = if state.setup.from_settings {
        select_and_back(state.regional.locale, "OK")
    } else {
        footer_hints(state.regional.locale, &[(FooterKey::Select, "OK")])
    };
    draw_footer(display, state, &hint)
}

/// What each key does, then "next" and "skip the setup".
fn draw_keys(display: &mut OrientedFrameBuffer<'_>, state: &AppState) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let preferences = state.display;
    draw_header(display, state, t(locale, "THE KEYS", "I TASTI"))?;
    let keys: [(&str, &str); 5] = [
        (
            t(locale, "Rocker up / down", "Rotella su / gi\u{00F9}"),
            t(
                locale,
                "Moves through menus, turns pages",
                "Scorre i menu, gira pagina",
            ),
        ),
        (
            t(
                locale,
                "Press the rocker (SELECT)",
                "Premi la rotella (SELECT)",
            ),
            t(locale, "Opens or confirms", "Apre o conferma"),
        ),
        (
            t(locale, "Hold the rocker down", "Tieni premuta la rotella"),
            t(
                locale,
                "Options of what is selected",
                "Opzioni di ci\u{00F2} che \u{00E8} selezionato",
            ),
        ),
        (
            t(locale, "BOOT key", "Tasto BOOT"),
            t(locale, "Goes back", "Torna indietro"),
        ),
        (
            t(locale, "Power key", "Tasto Power"),
            t(
                locale,
                "Standby; held down, opens a menu",
                "Standby; tenuto premuto apre un menu",
            ),
        ),
    ];
    let mut top = FIRST_ROW_TOP;
    for (title, detail) in keys {
        draw_tall_row(display, preferences, top, title, detail, "", false)?;
        top += TALL_ROW_STEP;
    }
    top += 8;
    let rows = [
        t(locale, "Next", "Avanti"),
        t(locale, "Skip the setup", "Salta la configurazione"),
    ];
    for (index, row) in rows.iter().enumerate() {
        draw_list_row(
            display,
            preferences,
            top,
            row,
            "",
            index == state.setup.selected,
        )?;
        top += ROW_STEP;
    }
    draw_page_footer(display, state)
}

fn draw_wifi(display: &mut OrientedFrameBuffer<'_>, state: &AppState) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let ready = state.setup_wifi_ready();
    let network = if state.wifi_connected() {
        state.network.ssid_label()
    } else {
        t(locale, "saved, not connected", "salvata, non collegata")
    };
    let fields = [(t(locale, "Wi-Fi network", "Rete Wi-Fi"), network)];
    let text = if ready {
        t(
            locale,
            "A network is already set up. Wi-Fi is for uploading books from a phone, keeping the clock right and getting updates.",
            "Una rete \u{00E8} gi\u{00E0} configurata. Il Wi-Fi serve per caricare libri dal telefono, tenere giusto l'orologio e ricevere gli aggiornamenti.",
        )
    } else {
        t(
            locale,
            "Wi-Fi is for uploading books from a phone, keeping the clock right and getting updates. Reading does not need it. You set it up from a phone, with a QR code.",
            "Il Wi-Fi serve per caricare libri dal telefono, tenere giusto l'orologio e ricevere gli aggiornamenti. Per leggere non serve. Si configura dal telefono, con un codice QR.",
        )
    };
    draw_text_page(
        display,
        state,
        "WI-FI",
        t(locale, "Connect to Wi-Fi", "Collega il Wi-Fi"),
        text,
        if ready { &fields[..] } else { &[] },
        &[
            t(locale, "Set up from a phone", "Configura dal telefono"),
            if ready {
                t(locale, "Next", "Avanti")
            } else {
                t(locale, "Skip", "Salta")
            },
        ],
    )
}

fn draw_clock(display: &mut OrientedFrameBuffer<'_>, state: &AppState) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let date = state.board.rtc.map_or_else(
        || t(locale, "not available", "non disponibile").to_string(),
        |rtc| long_date(locale, state.regional.localize_rtc(rtc)),
    );
    let time = state.board.time_label(state.regional);
    draw_text_page(
        display,
        state,
        t(locale, "DATE AND TIME", "DATA E ORA"),
        t(locale, "Date and time", "Data e ora"),
        t(
            locale,
            "With Wi-Fi they set themselves. Check that they are right, time zone included: the reading statistics count the days by them.",
            "Con il Wi-Fi si regolano da sole. Controlla che siano giuste, fuso orario compreso: le statistiche di lettura contano i giorni con queste.",
        ),
        &[
            (t(locale, "Date", "Data"), date.as_str()),
            (t(locale, "Time", "Ora"), time.as_str()),
            (
                t(locale, "Time zone", "Fuso orario"),
                state.regional.timezone_name(),
            ),
        ],
        &[
            t(locale, "They are right", "Vanno bene"),
            t(locale, "Change", "Cambia"),
        ],
    )
}

fn draw_book(display: &mut OrientedFrameBuffer<'_>, state: &AppState) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let text = if state.setup.from_settings {
        t(
            locale,
            "Books are EPUB or TXT files. There are two ways to copy them onto the device.",
            "I libri sono file EPUB o TXT. Ci sono due modi per copiarli sul dispositivo.",
        )
    } else {
        t(
            locale,
            "The Library already holds the \"Quick guide\", to try reading at once. Your own EPUB or TXT books can be copied in two ways.",
            "Nella Libreria c'\u{00E8} gi\u{00E0} la \u{00AB}Guida rapida\u{00BB}, per provare subito. I tuoi libri EPUB o TXT si copiano in due modi.",
        )
    };
    draw_text_page(
        display,
        state,
        t(locale, "FIRST BOOK", "PRIMO LIBRO"),
        t(locale, "Add your books", "Aggiungi i tuoi libri"),
        text,
        &[],
        &[
            t(
                locale,
                "From a phone, over Wi-Fi",
                "Dal telefono, via Wi-Fi",
            ),
            t(
                locale,
                "From a computer, by USB cable",
                "Dal computer, con il cavo USB",
            ),
            if state.setup.upload_started {
                t(locale, "Next", "Avanti")
            } else {
                t(locale, "Later", "Pi\u{00F9} tardi")
            },
        ],
    )
}

fn draw_done(display: &mut OrientedFrameBuffer<'_>, state: &AppState) -> Result<(), Infallible> {
    let locale = state.regional.locale;
    let text = if state.setup.from_settings {
        t(
            locale,
            "These pages are always in Settings, under \"First steps\".",
            "Queste pagine sono sempre in Opzioni, alla voce \u{00AB}Primi passi\u{00BB}.",
        )
    } else {
        t(
            locale,
            "These pages will not show at power-on again. You find them whenever you like in Settings, under \"First steps\".",
            "Queste pagine non compariranno pi\u{00F9} all'accensione. Le ritrovi quando vuoi in Opzioni, alla voce \u{00AB}Primi passi\u{00BB}.",
        )
    };
    draw_text_page(
        display,
        state,
        t(locale, "READY", "PRONTO"),
        t(locale, "All set", "Tutto pronto"),
        text,
        &[],
        &[t(locale, "Go to Home", "Vai alla Home")],
    )
}

/// Draw the first-run page the setup is on.
pub fn render_setup(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    match state.setup.page {
        SetupPage::Language => draw_language(display, state),
        SetupPage::Keys => draw_keys(display, state),
        SetupPage::Wifi => draw_wifi(display, state),
        SetupPage::Clock => draw_clock(display, state),
        SetupPage::Book => draw_book(display, state),
        SetupPage::Done => draw_done(display, state),
    }
}

/// The microSD is missing or cannot be read. The language is kept on the
/// card, so without it the warning cannot know which one to use and says
/// everything in both.
pub fn render_card_warning(
    display: &mut OrientedFrameBuffer<'_>,
    state: &AppState,
) -> Result<(), Infallible> {
    let preferences = state.display;
    let body = preferences.body_style();
    draw_header(display, state, "MICROSD")?;
    let next = draw_section_title(
        display,
        preferences,
        FIRST_BASELINE,
        "Memory card not readable",
    )?;
    let after = draw_paragraph(
        display,
        "Books and settings are on the microSD. Check that it is pushed all the way in, then restart. A new card of 64 GB or more must first be formatted as FAT32 on a computer.",
        CONTENT_LEFT,
        next,
        body,
        CONTENT_WIDTH,
        6,
        4,
    )?;
    let next = draw_section_title(
        display,
        preferences,
        after + 14,
        "Scheda di memoria non leggibile",
    )?;
    let after = draw_paragraph(
        display,
        "Libri e impostazioni stanno sulla microSD. Controlla che sia inserita fino in fondo, poi riavvia. Una scheda nuova da 64 GB o pi\u{00F9} va prima formattata in FAT32 dal computer.",
        CONTENT_LEFT,
        next,
        body,
        CONTENT_WIDTH,
        6,
        4,
    )?;
    let top = after + ROWS_GAP;
    draw_list_row(
        display,
        preferences,
        top,
        "Restart \u{00B7} Riavvia",
        "",
        state.card_warning_selected == 0,
    )?;
    draw_list_row(
        display,
        preferences,
        top + ROW_STEP,
        "Go on without it \u{00B7} Continua senza",
        "",
        state.card_warning_selected == 1,
    )?;
    draw_footer(
        display,
        state,
        &footer_hints(state.regional.locale, &[(FooterKey::Select, "OK")]),
    )
}

#[cfg(test)]
mod tests {
    use super::{render_card_warning, render_setup};
    use crate::{
        app::{setup::SetupPage, AppState},
        framebuffer::FrameBuffer,
        orientation::OrientedFrameBuffer,
        regional::Locale,
    };

    #[test]
    fn every_page_renders_in_both_languages() {
        for locale in [Locale::English, Locale::Italian] {
            for page in [
                SetupPage::Language,
                SetupPage::Keys,
                SetupPage::Wifi,
                SetupPage::Clock,
                SetupPage::Book,
                SetupPage::Done,
            ] {
                let mut frame = FrameBuffer::new_white();
                let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
                let mut state = AppState::default();
                state.regional.locale = locale;
                state.begin_first_run_setup(page.index());
                render_setup(&mut display, &state).unwrap();
            }
        }
        let mut frame = FrameBuffer::new_white();
        let mut display = OrientedFrameBuffer::new(&mut frame, Default::default());
        let mut state = AppState::default();
        state.show_card_warning();
        render_card_warning(&mut display, &state).unwrap();
    }
}
