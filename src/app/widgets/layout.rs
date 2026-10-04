//! Shared layout metrics of the portrait shell, so every screen starts,
//! ends and aligns its content at the same places as the Home dashboard.

/// Left edge of screen content.
pub const CONTENT_LEFT: i32 = 22;
/// Width of screen content.
pub const CONTENT_WIDTH: i32 = 436;
/// Right edge of screen content (exclusive).
pub const CONTENT_RIGHT: i32 = CONTENT_LEFT + CONTENT_WIDTH;
/// Top of the first element below the shared header.
pub const CONTENT_TOP: i32 = 58;
/// Last pixel row content may use above the footer.
pub const CONTENT_BOTTOM: i32 = 736;
/// Logical portrait screen width.
pub const SCREEN_WIDTH: i32 = 480;
/// Baseline of a screen's first section title or line of text.
pub const FIRST_BASELINE: i32 = CONTENT_TOP + 34;
/// Top of the first row of a screen that starts with a list.
pub const FIRST_ROW_TOP: i32 = CONTENT_TOP + 8;
