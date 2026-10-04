//! Text that fits: truncation and word wrap measured in pixels, shared by
//! every screen. `Text::draw` itself knows nothing about the space it has,
//! so anything that is not a short fixed label goes through here.

use core::convert::Infallible;

use embedded_graphics::prelude::Point;

use crate::{
    app::typography::{Text, UiTextStyle},
    orientation::OrientedFrameBuffer,
};

/// Appended to text cut short.
pub const ELLIPSIS: &str = "\u{2026}";

/// `text` cut to at most `max_width` pixels, ending in an ellipsis when
/// something was left out. Text that fits comes back unchanged.
#[must_use]
pub fn truncate_to_width(style: UiTextStyle, text: &str, max_width: i32) -> String {
    if style.text_width(text) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut candidate: String = chars.iter().collect::<String>().trim_end().to_string();
        candidate.push_str(ELLIPSIS);
        if style.text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

/// `text` cut from the left to at most `max_width` pixels, starting with an
/// ellipsis when something was left out: for paths, whose end matters most.
#[must_use]
pub fn truncate_start_to_width(style: UiTextStyle, text: &str, max_width: i32) -> String {
    if style.text_width(text) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for start in 1..chars.len() {
        let candidate = format!(
            "{ELLIPSIS}{}",
            chars[start..].iter().collect::<String>().trim_start()
        );
        if style.text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    String::new()
}

/// Greedy word wrap to `max_width` pixels and at most `max_lines` lines.
/// Explicit `\n` start a new line. A single word wider than the line is
/// broken across lines; when text is left over, the last line ends in an
/// ellipsis.
#[must_use]
pub fn wrap_to_width(
    style: UiTextStyle,
    text: &str,
    max_width: i32,
    max_lines: usize,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    if max_lines == 0 || max_width <= 0 {
        return lines;
    }
    let mut overflowed = false;
    'paragraphs: for paragraph in text.split('\n') {
        if lines.len() >= max_lines {
            overflowed = overflowed || !paragraph.trim().is_empty();
            break;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if style.text_width(&candidate) <= max_width {
                current = candidate;
                continue;
            }
            if !current.is_empty() {
                if lines.len() + 1 == max_lines {
                    current = candidate;
                    overflowed = true;
                    lines.push(current);
                    break 'paragraphs;
                }
                lines.push(std::mem::take(&mut current));
            }
            // The word alone: whole if it fits, else broken by characters.
            for character in word.chars() {
                current.push(character);
                if style.text_width(&current) > max_width && current.chars().count() > 1 {
                    current.pop();
                    if lines.len() + 1 == max_lines {
                        current.push(character);
                        overflowed = true;
                        lines.push(current);
                        break 'paragraphs;
                    }
                    lines.push(std::mem::take(&mut current));
                    current.push(character);
                }
            }
        }
        if !current.is_empty() {
            if lines.len() == max_lines {
                overflowed = true;
                break;
            }
            lines.push(current);
        }
    }
    if let Some(last) = lines.last_mut() {
        if overflowed {
            let mut marked = last.clone();
            marked.push_str(ELLIPSIS);
            *last = truncate_marked(style, &marked, max_width);
        } else {
            *last = truncate_to_width(style, last, max_width);
        }
    }
    lines
}

/// Like [`truncate_to_width`], for a line that must end in an ellipsis even
/// when it already fits.
fn truncate_marked(style: UiTextStyle, marked: &str, max_width: i32) -> String {
    if style.text_width(marked) <= max_width {
        return marked.to_string();
    }
    let body = marked.strip_suffix(ELLIPSIS).unwrap_or(marked);
    let mut chars: Vec<char> = body.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let mut candidate: String = chars.iter().collect::<String>().trim_end().to_string();
        candidate.push_str(ELLIPSIS);
        if style.text_width(&candidate) <= max_width {
            return candidate;
        }
    }
    ELLIPSIS.to_string()
}

/// One line, truncated to `max_width`.
pub fn draw_text_fit(
    display: &mut OrientedFrameBuffer<'_>,
    text: &str,
    baseline: Point,
    style: UiTextStyle,
    max_width: i32,
) -> Result<(), Infallible> {
    Text::new(&truncate_to_width(style, text, max_width), baseline, style).draw(display)?;
    Ok(())
}

/// One line ending at `right`, truncated to `max_width`.
pub fn draw_text_right(
    display: &mut OrientedFrameBuffer<'_>,
    text: &str,
    right: i32,
    baseline_y: i32,
    style: UiTextStyle,
    max_width: i32,
) -> Result<(), Infallible> {
    let fitted = truncate_to_width(style, text, max_width);
    let left = right - style.text_width(&fitted);
    Text::new(&fitted, Point::new(left, baseline_y), style).draw(display)?;
    Ok(())
}

/// One line centered between `left` and `left + width`, truncated to fit.
pub fn draw_text_centered(
    display: &mut OrientedFrameBuffer<'_>,
    text: &str,
    left: i32,
    width: i32,
    baseline_y: i32,
    style: UiTextStyle,
) -> Result<(), Infallible> {
    let fitted = truncate_to_width(style, text, width);
    let x = left + (width - style.text_width(&fitted)) / 2;
    Text::new(&fitted, Point::new(x, baseline_y), style).draw(display)?;
    Ok(())
}

/// A wrapped paragraph whose first baseline is `first_baseline`. Returns the
/// baseline the next line would take, so callers can stack blocks.
#[allow(clippy::too_many_arguments)]
pub fn draw_paragraph(
    display: &mut OrientedFrameBuffer<'_>,
    text: &str,
    left: i32,
    first_baseline: i32,
    style: UiTextStyle,
    max_width: i32,
    max_lines: usize,
    line_gap: i32,
) -> Result<i32, Infallible> {
    let step = i32::from(style.line_height()) + line_gap;
    let mut baseline = first_baseline;
    for line in wrap_to_width(style, text, max_width, max_lines) {
        Text::new(&line, Point::new(left, baseline), style).draw(display)?;
        baseline += step;
    }
    Ok(baseline)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::BinaryColor;

    use super::{truncate_start_to_width, truncate_to_width, wrap_to_width, ELLIPSIS};
    use crate::app::{
        display::UiFontSize,
        typography::{style_for, UiTextRole},
    };

    fn body() -> crate::app::typography::UiTextStyle {
        style_for(UiFontSize::Standard, UiTextRole::Body, BinaryColor::On)
    }

    #[test]
    fn text_that_fits_is_left_alone() {
        assert_eq!(truncate_to_width(body(), "Rete", 200), "Rete");
        assert_eq!(wrap_to_width(body(), "Rete", 200, 3), vec!["Rete"]);
    }

    #[test]
    fn truncation_ends_in_an_ellipsis_and_fits() {
        let style = body();
        let cut = truncate_to_width(style, "Un nome di file davvero molto lungo", 120);
        assert!(cut.ends_with(ELLIPSIS));
        assert!(style.text_width(&cut) <= 120);
        let cut = truncate_start_to_width(style, "/sdcard/RUSTMIX/BOOKS/Narrativa", 120);
        assert!(cut.starts_with(ELLIPSIS) && cut.ends_with("Narrativa"));
        assert!(style.text_width(&cut) <= 120);
    }

    #[test]
    fn wrapping_never_exceeds_the_width_or_the_line_count() {
        let style = body();
        let text = "Il testo dell'interfaccia va a capo quando non entra nella riga disponibile";
        let lines = wrap_to_width(style, text, 160, 3);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| style.text_width(line) <= 160));
        assert!(lines[2].ends_with(ELLIPSIS));
        let all = wrap_to_width(style, text, 160, usize::MAX);
        assert_eq!(all.join(" "), text);
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken() {
        let style = body();
        let lines = wrap_to_width(
            style,
            "ESP_ERR_TIMEOUT_WHILE_READING_SECTOR",
            80,
            usize::MAX,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| style.text_width(line) <= 80));
        assert_eq!(lines.concat(), "ESP_ERR_TIMEOUT_WHILE_READING_SECTOR");
    }

    #[test]
    fn explicit_newlines_start_new_lines() {
        assert_eq!(
            wrap_to_width(body(), "uno\ndue", 300, usize::MAX),
            vec!["uno", "due"]
        );
    }
}
