//! Reader-specific body typography.
//!
//! Reader pages deliberately use an independent font preference boundary so
//! global UI typography remains stable. Inter reuses the existing UI strike;
//! Atkinson Hyperlegible Next Medium, DejaVu Serif and Literata Medium use
//! generated printable-ASCII Reader-only bitmap strikes. TXT normalization
//! converts unsupported punctuation before layout.

use embedded_graphics::pixelcolor::BinaryColor;

use super::{
    display::{UiFontFamily, UiFontSize},
    reader_atkinson_next_assets::{
        ATKINSON_NEXT_LARGE, ATKINSON_NEXT_XLARGE, ATKINSON_NEXT_XXLARGE, ATKINSON_NEXT_XXXLARGE,
    },
    reader_literata_assets::{LITERATA_LARGE, LITERATA_XLARGE, LITERATA_XXLARGE, LITERATA_XXXLARGE},
    reader_serif_assets::{SERIF_LARGE, SERIF_XLARGE, SERIF_XXLARGE, SERIF_XXXLARGE},
    typography::{style_for, UiTextRole, UiTextStyle},
};
use crate::reader::{BookFont, BookFontSize, ReadingTheme};

/// Resolve one Reader body strike without affecting global UI preferences.
#[must_use]
pub const fn reader_body_style(
    family: BookFont,
    size: BookFontSize,
    _theme: ReadingTheme,
) -> UiTextStyle {
    match family {
        BookFont::Inter => style_for(
            UiFontFamily::Inter,
            ui_profile(size),
            ui_role(size),
            BinaryColor::On,
        ),
        BookFont::AtkinsonHyperlegible => {
            UiTextStyle::new(atkinson_next_font(size), BinaryColor::On)
        }
        BookFont::Serif => UiTextStyle::new(serif_font(size), BinaryColor::On),
        BookFont::Literata => UiTextStyle::new(literata_font(size), BinaryColor::On),
    }
}

#[must_use]
const fn ui_profile(size: BookFontSize) -> UiFontSize {
    match size {
        BookFontSize::Large => UiFontSize::Standard,
        // Inter reuses the shared UI type system, which tops out at
        // `UiFontSize::Large`. XLarge, XXLarge and XXXLarge all render at
        // that ceiling (distinguished only by `ui_role` below) because
        // growing Inter further would mean adding a new tier to the
        // global UI typography system used everywhere else in the app,
        // not just the Reader's book-font picker.
        BookFontSize::XLarge | BookFontSize::XXLarge | BookFontSize::XXXLarge => {
            UiFontSize::Large
        }
    }
}

#[must_use]
const fn ui_role(size: BookFontSize) -> UiTextRole {
    match size {
        BookFontSize::Large => UiTextRole::Body,
        BookFontSize::XLarge | BookFontSize::XXLarge | BookFontSize::XXXLarge => {
            UiTextRole::Heading
        }
    }
}

#[must_use]
const fn atkinson_next_font(size: BookFontSize) -> &'static super::typography::BitmapFont {
    match size {
        BookFontSize::Large => &ATKINSON_NEXT_LARGE,
        BookFontSize::XLarge => &ATKINSON_NEXT_XLARGE,
        BookFontSize::XXLarge => &ATKINSON_NEXT_XXLARGE,
        BookFontSize::XXXLarge => &ATKINSON_NEXT_XXXLARGE,
    }
}

#[must_use]
const fn serif_font(size: BookFontSize) -> &'static super::typography::BitmapFont {
    match size {
        BookFontSize::Large => &SERIF_LARGE,
        BookFontSize::XLarge => &SERIF_XLARGE,
        BookFontSize::XXLarge => &SERIF_XXLARGE,
        BookFontSize::XXXLarge => &SERIF_XXXLARGE,
    }
}

#[must_use]
const fn literata_font(size: BookFontSize) -> &'static super::typography::BitmapFont {
    match size {
        BookFontSize::Large => &LITERATA_LARGE,
        BookFontSize::XLarge => &LITERATA_XLARGE,
        BookFontSize::XXLarge => &LITERATA_XXLARGE,
        BookFontSize::XXXLarge => &LITERATA_XXXLARGE,
    }
}

#[cfg(test)]
mod tests {
    use super::reader_body_style;
    use crate::reader::{BookFont, BookFontSize, ReadingTheme};

    #[test]
    fn resolves_all_reader_body_profiles() {
        for family in [
            BookFont::Inter,
            BookFont::AtkinsonHyperlegible,
            BookFont::Serif,
            BookFont::Literata,
        ] {
            for size in [
                BookFontSize::Large,
                BookFontSize::XLarge,
                BookFontSize::XXLarge,
                BookFontSize::XXXLarge,
            ] {
                assert!(reader_body_style(family, size, ReadingTheme::Classic).line_height() > 0);
            }
        }
    }
}
