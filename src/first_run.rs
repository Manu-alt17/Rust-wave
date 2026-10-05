//! What the firmware does for a microSD it has never seen: make the folders
//! it expects, tell a new card from a used one, remember how far the
//! first-run pages got, and leave a short guide to read.
//!
//! Everything here is plain file work under one root, so it runs on the
//! host as well as on the device. The pages themselves are in
//! `app::setup` and `app::screens::setup`.
//!
//! The rule the whole module follows: a card is "new" only when it was read
//! without a single error and holds neither settings nor books. Anything
//! that could not be read counts as "not new", so a bad contact can never
//! pass for an empty card.

use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use crate::regional::Locale;

/// Folder that holds everything the firmware keeps on the card.
const DATA_DIRECTORY: &str = "RUSTMIX";
/// Folders made on every boot when they are missing: books, audiobooks and
/// sleep-screen images.
const CONTENT_DIRECTORIES: [&str; 3] = ["BOOKS", "AUDIO", "SLEEP"];
/// Where the first-run pages keep how far they got.
const MARKER_FILE: &str = "SETUP.TXT";
/// Settings files whose presence means the card was already used.
const SETTINGS_FILES: [&str; 7] = [
    "WIFI.TXT",
    "CLOCK.TXT",
    "DISPLAY.TXT",
    "MENU.TXT",
    "UPDATE.TXT",
    "VOLUME.TXT",
    "AUDIOPOS.TXT",
];
/// Folders that mean the card was already used as soon as they hold
/// anything.
const USED_DIRECTORIES: [&str; 4] = ["BOOKS", "AUDIO", "READER", "STATS"];
/// The two "read me" files, one per language.
const README_ITALIAN: &str = "LEGGIMI.TXT";
const README_ENGLISH: &str = "README.TXT";

/// What the first-run pages should do on this boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CardSetup {
    /// A card read without errors, with no settings and no books: show the
    /// pages from the first one.
    New,
    /// The pages were left at this one (see `app::setup::SetupPage::index`).
    Resume(u8),
    /// A card already in use that carries no marker yet, as every card
    /// written by a firmware older than these pages: nothing to show, and
    /// the caller notes it on the card so the next boots need one read.
    Used,
    /// Nothing to show: the pages were finished, or the card could not be
    /// read reliably.
    Done,
}

/// How far the first-run pages got, as kept in the marker file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupProgress {
    Page(u8),
    Done,
}

fn data_directory(root: &Path) -> PathBuf {
    root.join(DATA_DIRECTORY)
}

fn marker_path(root: &Path) -> PathBuf {
    data_directory(root).join(MARKER_FILE)
}

/// Makes the folders the firmware expects, when they are missing. Returns
/// whether the books folder had to be made, which is when the "read me"
/// files are worth writing (see [`write_readme`]).
pub fn prepare_card(root: impl AsRef<Path>) -> io::Result<bool> {
    let data = data_directory(root.as_ref());
    let books_missing = match fs::metadata(data.join(CONTENT_DIRECTORIES[0])) {
        Ok(_) => false,
        Err(error) if error.kind() == ErrorKind::NotFound => true,
        Err(error) => return Err(error),
    };
    for (index, directory) in CONTENT_DIRECTORIES.into_iter().enumerate() {
        let path = data.join(directory);
        // One look per folder on a card that already has them, which is
        // every boot but the first.
        let missing = if index == 0 {
            books_missing
        } else {
            !exists(&path)?
        };
        if missing {
            fs::create_dir_all(path)?;
        }
    }
    Ok(books_missing)
}

/// Whether `path` exists: `Ok(false)` only when the card answered "not
/// found", an error for anything else.
fn exists(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether the folder at `path` holds anything. A missing folder is empty.
fn holds_anything(path: &Path) -> io::Result<bool> {
    match fs::read_dir(path) {
        Ok(mut entries) => match entries.next() {
            Some(Ok(_)) => Ok(true),
            Some(Err(error)) => Err(error),
            None => Ok(false),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn parse_marker(text: &str) -> Option<SetupProgress> {
    let mut progress = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("done", "1") => return Some(SetupProgress::Done),
            ("page", value) => {
                if let Ok(page) = value.parse::<u8>() {
                    progress = Some(SetupProgress::Page(page));
                }
            }
            _ => {}
        }
    }
    progress
}

/// What one pass over the card found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Inspection {
    /// The marker file, and what it says.
    Marker(SetupProgress),
    /// No marker, but settings or books.
    Used,
    /// No marker, no settings, no books.
    New,
}

/// One pass over the card.
fn inspect(root: &Path) -> io::Result<Inspection> {
    // The card's top folder must list: a card that cannot do that much is
    // not one to draw conclusions from.
    fs::read_dir(root)?;
    match fs::read_to_string(marker_path(root)) {
        // A marker nobody can make sense of was not written by a finished
        // or interrupted setup; showing the pages again would only annoy.
        Ok(text) => {
            return Ok(Inspection::Marker(
                parse_marker(&text).unwrap_or(SetupProgress::Done),
            ))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let data = data_directory(root);
    for file in SETTINGS_FILES {
        if exists(&data.join(file))? {
            return Ok(Inspection::Used);
        }
    }
    for directory in USED_DIRECTORIES {
        if holds_anything(&data.join(directory))? {
            return Ok(Inspection::Used);
        }
    }
    Ok(Inspection::New)
}

/// Decides what the first-run pages do on this boot. The card is read
/// twice and called new only when both passes agree; any error, on either
/// pass, means [`CardSetup::Done`].
#[must_use]
pub fn card_setup(root: impl AsRef<Path>) -> CardSetup {
    let root = root.as_ref();
    match inspect(root) {
        Ok(Inspection::Marker(SetupProgress::Done)) | Err(_) => CardSetup::Done,
        Ok(Inspection::Marker(SetupProgress::Page(page))) => CardSetup::Resume(page),
        Ok(Inspection::Used) => CardSetup::Used,
        Ok(Inspection::New) => match inspect(root) {
            Ok(Inspection::New) => CardSetup::New,
            _ => CardSetup::Done,
        },
    }
}

/// Whether the books folder was read and holds nothing. `false` when it
/// could not be read: only a folder known to be empty counts.
#[must_use]
pub fn books_folder_is_empty(root: impl AsRef<Path>) -> bool {
    let books = data_directory(root.as_ref()).join(CONTENT_DIRECTORIES[0]);
    matches!(
        fs::read_dir(books).map(|mut entries| entries.next()),
        Ok(None)
    )
}

/// Writes how far the first-run pages got.
pub fn save_progress(root: impl AsRef<Path>, progress: SetupProgress) -> io::Result<()> {
    let root = root.as_ref();
    fs::create_dir_all(data_directory(root))?;
    let line = match progress {
        SetupProgress::Page(page) => format!("page={page}"),
        SetupProgress::Done => "done=1".to_string(),
    };
    fs::write(
        marker_path(root),
        format!("# RustMix Wave first-run setup\n{line}\n"),
    )
}

/// Writes `text` to `path` unless a file is already there. Returns whether
/// it wrote.
fn write_if_missing(path: &Path, text: &str) -> io::Result<bool> {
    if exists(path)? {
        return Ok(false);
    }
    fs::write(path, text)?;
    Ok(true)
}

/// Leaves the two "read me" files in the data folder, in Italian and in
/// English. Files already there are never replaced.
pub fn write_readme(root: impl AsRef<Path>) -> io::Result<()> {
    let data = data_directory(root.as_ref());
    fs::create_dir_all(&data)?;
    write_if_missing(&data.join(README_ITALIAN), README_TEXT_ITALIAN)?;
    write_if_missing(&data.join(README_ENGLISH), README_TEXT_ENGLISH)?;
    Ok(())
}

/// File name of the quick guide in `locale`, inside the books folder.
#[must_use]
pub const fn quick_guide_file_name(locale: Locale) -> &'static str {
    match locale {
        Locale::English => "Quick guide.txt",
        Locale::Italian => "Guida rapida.txt",
    }
}

/// Puts the quick guide in the books folder as a first book, in `locale`.
/// A file already there is never replaced; returns whether it wrote.
pub fn write_quick_guide(root: impl AsRef<Path>, locale: Locale) -> io::Result<bool> {
    let books = data_directory(root.as_ref()).join(CONTENT_DIRECTORIES[0]);
    fs::create_dir_all(&books)?;
    let text = match locale {
        Locale::English => QUICK_GUIDE_ENGLISH,
        Locale::Italian => QUICK_GUIDE_ITALIAN,
    };
    write_if_missing(&books.join(quick_guide_file_name(locale)), text)
}

const README_TEXT_ITALIAN: &str = "\
RUSTMIX WAVE - COSA C'E' IN QUESTA CARTELLA\r\n\
\r\n\
Questa cartella e' usata dal lettore. Le sottocartelle da riempire sono tre:\r\n\
\r\n\
BOOKS   i libri, in formato EPUB o TXT\r\n\
AUDIO   gli audiolibri in MP3: un file per titolo, oppure una cartella\r\n\
        per titolo con dentro le tracce\r\n\
SLEEP   le immagini mostrate quando il dispositivo e' in standby\r\n\
\r\n\
Per copiare i file puoi:\r\n\
- collegare il dispositivo al computer: Home > Carica > Cavo USB;\r\n\
- usare il telefono, senza cavo: Home > Carica > Wi-Fi;\r\n\
- togliere la scheda e leggerla dal computer.\r\n\
\r\n\
Gli altri file (WIFI.TXT, CLOCK.TXT, DISPLAY.TXT, la cartella READER e\r\n\
simili) sono impostazioni e posizioni di lettura: non serve toccarli.\r\n\
Cancellarli riporta il dispositivo alle impostazioni iniziali.\r\n\
\r\n\
La scheda deve essere formattata in FAT32 (non exFAT).\r\n";

const README_TEXT_ENGLISH: &str = "\
RUSTMIX WAVE - WHAT IS IN THIS FOLDER\r\n\
\r\n\
This folder is used by the reader. There are three folders to fill:\r\n\
\r\n\
BOOKS   books, as EPUB or TXT files\r\n\
AUDIO   MP3 audiobooks: one file per title, or one folder per title\r\n\
        with its tracks inside\r\n\
SLEEP   images shown while the device is in standby\r\n\
\r\n\
To copy files you can:\r\n\
- connect the device to a computer: Home > Upload > USB cable;\r\n\
- use a phone, without a cable: Home > Upload > Wi-Fi;\r\n\
- take the card out and read it from a computer.\r\n\
\r\n\
The other files (WIFI.TXT, CLOCK.TXT, DISPLAY.TXT, the READER folder and\r\n\
the like) are settings and reading positions: there is no need to touch\r\n\
them. Deleting them puts the device back to its first settings.\r\n\
\r\n\
The card must be formatted as FAT32 (not exFAT).\r\n";

const QUICK_GUIDE_ITALIAN: &str = "\
GUIDA RAPIDA\n\
\n\
Questo \u{00E8} un libro di prova: serve a vedere com'\u{00E8} leggere su questo dispositivo e a tenere a portata di mano i comandi. Quando non ti serve pi\u{00F9} puoi eliminarlo dalla Libreria.\n\
\n\
\n\
I TASTI\n\
\n\
Rotella su e gi\u{00F9}: scorre i menu e gira le pagine. Gi\u{00F9} va alla pagina dopo, su a quella prima.\n\
\n\
Rotella premuta (SELECT): apre ci\u{00F2} che \u{00E8} selezionato o conferma.\n\
\n\
Rotella tenuta premuta: apre le opzioni di ci\u{00F2} che \u{00E8} selezionato. Nella Libreria sono le azioni sul libro, mentre leggi sono le opzioni di lettura.\n\
\n\
Tasto BOOT: torna indietro di un passo.\n\
\n\
Tasto Power, premuto una volta: mette il dispositivo in standby. Premilo ancora per riprendere da dove eri.\n\
\n\
Tasto Power, tenuto premuto: apre un piccolo menu per ripulire lo schermo o riavviare.\n\
\n\
In fondo a ogni schermata trovi scritti i tasti che servono l\u{00EC}.\n\
\n\
\n\
MENTRE LEGGI\n\
\n\
Tieni premuta la rotella per aprire le opzioni: indice, segnalibri, \u{00AB}Vai a\u{00BB} e preferenze di lettura. Nelle preferenze cambi la dimensione e il tipo di carattere, l'allineamento, l'orientamento e il tema.\n\
\n\
Premi la rotella su una pagina per cercare una parola nel dizionario, se ne hai installato uno.\n\
\n\
La posizione si salva da sola a ogni pagina. Dalla Home, \u{00AB}Continua\u{00BB} riapre l'ultimo libro dove lo avevi lasciato.\n\
\n\
\n\
AGGIUNGERE LIBRI\n\
\n\
I libri sono file EPUB o TXT nella cartella RUSTMIX/BOOKS della scheda di memoria. Dalla Home apri \u{00AB}Carica\u{00BB} e scegli come copiarli.\n\
\n\
Wi-Fi: il dispositivo mostra un codice QR. Inquadralo con il telefono e si apre una pagina da cui caricare i libri. Se non hai ancora collegato una rete, la stessa pagina serve anche ad aggiungerla.\n\
\n\
Cavo USB: collega il dispositivo al computer e la scheda compare come un disco. \u{00C8} il modo pi\u{00F9} rapido per molti file. Alla fine il dispositivo si riavvia.\n\
\n\
\n\
AUDIOLIBRI\n\
\n\
Gli audiolibri sono file MP3 nella cartella RUSTMIX/AUDIO: un file per titolo, oppure una cartella per titolo con dentro le tracce. Nel lettore la rotella regola il volume, SELECT mette in pausa e la rotella tenuta premuta apre il menu.\n\
\n\
\n\
LO SCHERMO\n\
\n\
Lo schermo \u{00E8} di carta elettronica: consuma solo quando cambia pagina e non emette luce. Ogni tanto lampeggia per ripulirsi: \u{00E8} normale. Se restano ombre, tieni premuto Power e scegli di ripulire lo schermo.\n\
\n\
Dopo qualche minuto senza toccarlo il dispositivo va in standby da solo. Il tempo si cambia in Opzioni, Schermo.\n\
\n\
\n\
LE OPZIONI\n\
\n\
Dalla Home, \u{00AB}Opzioni\u{00BB} raccoglie rete Wi-Fi, aggiornamenti, audio, orologio, schermo, lingua e informazioni sul dispositivo. L\u{00EC} trovi anche \u{00AB}Primi passi\u{00BB}, per rivedere la configurazione iniziale.\n\
\n\
Buona lettura.\n";

const QUICK_GUIDE_ENGLISH: &str = "\
QUICK GUIDE\n\
\n\
This is a sample book: it shows what reading on this device is like and keeps the controls at hand. When you no longer need it you can delete it from the Library.\n\
\n\
\n\
THE KEYS\n\
\n\
Rocker up and down: moves through the menus and turns the pages. Down goes to the next page, up to the one before.\n\
\n\
Rocker pressed (SELECT): opens what is selected, or confirms.\n\
\n\
Rocker held down: opens the options of what is selected. In the Library these are the book's actions, while reading they are the reading options.\n\
\n\
BOOT key: goes back one step.\n\
\n\
Power key, pressed once: puts the device in standby. Press it again to pick up where you were.\n\
\n\
Power key, held down: opens a small menu to clean the screen or restart.\n\
\n\
The bottom of every screen names the keys that are useful there.\n\
\n\
\n\
WHILE READING\n\
\n\
Hold the rocker down to open the options: table of contents, bookmarks, \"Go to\" and the reading preferences. In the preferences you change the size and kind of type, the alignment, the orientation and the theme.\n\
\n\
Press the rocker on a page to look a word up in the dictionary, if you installed one.\n\
\n\
Your place is saved by itself at every page. From Home, \"Continue\" reopens the last book where you left it.\n\
\n\
\n\
ADDING BOOKS\n\
\n\
Books are EPUB or TXT files in the RUSTMIX/BOOKS folder of the memory card. From Home open \"Upload\" and choose how to copy them.\n\
\n\
Wi-Fi: the device shows a QR code. Point a phone at it and a page opens to upload books from. If no network is connected yet, the same page also adds one.\n\
\n\
USB cable: connect the device to a computer and the card shows up as a disk. It is the quickest way for many files. The device restarts when done.\n\
\n\
\n\
AUDIOBOOKS\n\
\n\
Audiobooks are MP3 files in the RUSTMIX/AUDIO folder: one file per title, or one folder per title with its tracks inside. In the player the rocker sets the volume, SELECT pauses and the rocker held down opens the menu.\n\
\n\
\n\
THE SCREEN\n\
\n\
The screen is electronic paper: it uses power only when the page changes and gives off no light. Now and then it flashes to clean itself: that is normal. If shadows stay, hold Power and choose to clean the screen.\n\
\n\
After a few minutes untouched the device goes into standby by itself. The time is set in Settings, Display.\n\
\n\
\n\
SETTINGS\n\
\n\
From Home, \"Settings\" gathers the Wi-Fi network, updates, audio, clock, display, language and device information. \"First steps\" is there too, to go through the first setup again.\n\
\n\
Enjoy your reading.\n";

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        books_folder_is_empty, card_setup, prepare_card, quick_guide_file_name, save_progress,
        write_quick_guide, write_readme, CardSetup, SetupProgress,
    };
    use crate::regional::Locale;

    fn temp_card(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rustmix-first-run-{name}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_card_gets_its_folders_and_counts_as_new() {
        let card = temp_card("new");
        // The boot log makes RUSTMIX before anything looks at the card.
        fs::create_dir_all(card.join("RUSTMIX")).unwrap();
        fs::write(card.join("RUSTMIX/BOOTTIME.LOG"), "boot\n").unwrap();
        assert!(prepare_card(&card).unwrap());
        for directory in ["BOOKS", "AUDIO", "SLEEP"] {
            assert!(card.join("RUSTMIX").join(directory).is_dir());
        }
        // Empty folders and logs are not use.
        assert_eq!(card_setup(&card), CardSetup::New);
        assert!(books_folder_is_empty(&card));
        // The folders are there now: nothing to make a second time.
        assert!(!prepare_card(&card).unwrap());
        fs::remove_dir_all(&card).unwrap();
    }

    #[test]
    fn settings_or_books_mean_the_card_was_used() {
        for used in ["WIFI.TXT", "CLOCK.TXT", "DISPLAY.TXT", "BOOKS/Libro.epub"] {
            let card = temp_card("used");
            prepare_card(&card).unwrap();
            fs::write(card.join("RUSTMIX").join(used), "x").unwrap();
            assert_eq!(card_setup(&card), CardSetup::Used, "{used}");
            fs::remove_dir_all(&card).unwrap();
        }
        let card = temp_card("used-reader");
        fs::create_dir_all(card.join("RUSTMIX/READER")).unwrap();
        fs::write(card.join("RUSTMIX/READER/STATE.TXT"), "x").unwrap();
        assert_eq!(card_setup(&card), CardSetup::Used);
        fs::remove_dir_all(&card).unwrap();
    }

    #[test]
    fn a_card_that_cannot_be_read_is_never_new() {
        let card = temp_card("missing");
        fs::remove_dir_all(&card).unwrap();
        // No such folder: the listing fails, as on a card that dropped out.
        assert_eq!(card_setup(&card), CardSetup::Done);
        assert!(!books_folder_is_empty(&card));
    }

    #[test]
    fn progress_survives_a_restart_and_the_guide_does_not_hide_it() {
        let card = temp_card("resume");
        prepare_card(&card).unwrap();
        save_progress(&card, SetupProgress::Page(2)).unwrap();
        // A book on the card would make it "used": the marker wins.
        assert!(write_quick_guide(&card, Locale::Italian).unwrap());
        assert!(!books_folder_is_empty(&card));
        assert_eq!(card_setup(&card), CardSetup::Resume(2));
        save_progress(&card, SetupProgress::Done).unwrap();
        assert_eq!(card_setup(&card), CardSetup::Done);
        // A marker with nothing usable in it never restarts the pages.
        fs::write(card.join("RUSTMIX/SETUP.TXT"), "garbage\n").unwrap();
        assert_eq!(card_setup(&card), CardSetup::Done);
        fs::remove_dir_all(&card).unwrap();
    }

    #[test]
    fn the_guide_shows_in_the_library_under_its_name() {
        let card = temp_card("library");
        prepare_card(&card).unwrap();
        write_quick_guide(&card, Locale::Italian).unwrap();
        write_quick_guide(&card, Locale::English).unwrap();
        let books = crate::reader::scan_txt_library(card.join("RUSTMIX/BOOKS"), &[]).unwrap();
        let titles: Vec<&str> = books.iter().map(|book| book.title.as_str()).collect();
        assert_eq!(titles, ["Guida rapida", "Quick guide"]);
        fs::remove_dir_all(&card).unwrap();
    }

    #[test]
    fn the_guide_and_the_readme_never_replace_a_file() {
        let card = temp_card("guide");
        prepare_card(&card).unwrap();
        let guide = card
            .join("RUSTMIX/BOOKS")
            .join(quick_guide_file_name(Locale::English));
        fs::write(&guide, "mine").unwrap();
        assert!(!write_quick_guide(&card, Locale::English).unwrap());
        assert_eq!(fs::read_to_string(&guide).unwrap(), "mine");
        assert!(write_quick_guide(&card, Locale::Italian).unwrap());

        fs::write(card.join("RUSTMIX/README.TXT"), "mine").unwrap();
        write_readme(&card).unwrap();
        assert_eq!(
            fs::read_to_string(card.join("RUSTMIX/README.TXT")).unwrap(),
            "mine"
        );
        assert!(fs::read_to_string(card.join("RUSTMIX/LEGGIMI.TXT"))
            .unwrap()
            .contains("BOOKS"));
        fs::remove_dir_all(&card).unwrap();
    }
}
