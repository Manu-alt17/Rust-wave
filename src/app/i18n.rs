//! Minimal translation helper for on-device UI text.
//!
//! Screens hold English and Italian text side by side at each call site
//! rather than indirecting through a key/catalog lookup, so a translation is
//! always visible next to the string it replaces and stays in sync with it.

use crate::regional::Locale;

/// Pick the text for `locale` between an English and an Italian literal.
#[must_use]
pub const fn t(locale: Locale, en: &'static str, it: &'static str) -> &'static str {
    match locale {
        Locale::English => en,
        Locale::Italian => it,
    }
}
