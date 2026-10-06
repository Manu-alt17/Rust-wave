//! The screen that chooses the fixed sleep wallpaper: Settings > Display >
//! Fixed wallpaper.
//!
//! The wallpapers of `/sdcard/RUSTMIX/SLEEP` are shown one at a time, as
//! standby will show them; their names (`SLEEP003.BMP`) say nothing, so the
//! picture itself is what is chosen. This is only the state: the runtime
//! owner in main.rs lists the folder, decodes the wallpaper under the cursor
//! and stores the choice, since all three read or write the card.

use crate::framebuffer::FrameBuffer;

/// State of the fixed-wallpaper chooser.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SleepPickerState {
    /// Wallpapers on the card, in the order the rotation shows them.
    names: Vec<String>,
    selected: usize,
    /// Whether the folder has been listed since the screen opened.
    listed: bool,
    /// The wallpaper under the cursor, decoded: its name and its frame.
    preview: Option<(String, FrameBuffer)>,
    /// A wallpaper that could not be decoded, so it is not asked for again.
    unreadable: Option<String>,
    /// The wallpaper standby shows now.
    current: Option<String>,
    /// Chosen with SELECT, until the runtime has stored it.
    chosen: Option<String>,
    /// The wallpaper last painted, so the runtime can tell a new picture
    /// from the same one drawn again.
    painted: Option<String>,
}

impl SleepPickerState {
    /// Start over for a new visit to the screen. A choice still waiting to
    /// be stored is kept.
    pub fn open(&mut self) {
        self.close();
    }

    /// Leave the screen: the list and the decoded wallpaper (48 KB) go.
    pub fn close(&mut self) {
        let chosen = self.chosen.take();
        *self = Self {
            chosen,
            ..Self::default()
        };
    }

    /// Whether the runtime still has to list the folder.
    #[must_use]
    pub const fn needs_listing(&self) -> bool {
        !self.listed
    }

    /// The folder's wallpapers and the one standby shows now, which is where
    /// the cursor starts.
    pub fn set_listing(&mut self, names: Vec<String>, current: Option<String>) {
        self.selected = current
            .as_deref()
            .and_then(|current| {
                names
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(current))
            })
            .unwrap_or(0);
        self.names = names;
        self.current = current;
        self.listed = true;
        self.preview = None;
        self.unreadable = None;
    }

    /// Whether the folder was listed and holds no wallpaper.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.listed && self.names.is_empty()
    }

    /// Name of the wallpaper under the cursor.
    #[must_use]
    pub fn selected_name(&self) -> Option<&str> {
        self.names.get(self.selected).map(String::as_str)
    }

    /// The wallpaper the runtime has to decode before the next paint, if
    /// the one under the cursor is not in memory yet.
    #[must_use]
    pub fn wanted_preview(&self) -> Option<&str> {
        let name = self.selected_name()?;
        let loaded = self
            .preview
            .as_ref()
            .is_some_and(|(loaded, _)| loaded == name);
        let unreadable = self.unreadable.as_deref() == Some(name);
        (!loaded && !unreadable).then_some(name)
    }

    /// The decoded wallpaper `name`, or `None` when it cannot be read.
    pub fn set_preview(&mut self, name: &str, frame: Option<FrameBuffer>) {
        match frame {
            Some(frame) => {
                self.preview = Some((name.to_string(), frame));
                self.unreadable = None;
            }
            None => {
                self.preview = None;
                self.unreadable = Some(name.to_string());
            }
        }
    }

    /// The wallpaper under the cursor, when it is decoded.
    #[must_use]
    pub fn preview(&self) -> Option<&FrameBuffer> {
        let name = self.selected_name()?;
        self.preview
            .as_ref()
            .filter(|(loaded, _)| loaded == name)
            .map(|(_, frame)| frame)
    }

    /// Whether the wallpaper about to be painted is another one than the
    /// last painted. Asked once per paint: it also records the answer.
    pub fn take_preview_changed(&mut self) -> bool {
        let shown = self.selected_name().map(str::to_string);
        let changed = shown != self.painted;
        self.painted = shown;
        changed
    }

    /// `(position, count)` of the cursor, counted from one.
    #[must_use]
    pub fn position(&self) -> Option<(usize, usize)> {
        (!self.names.is_empty()).then_some((self.selected + 1, self.names.len()))
    }

    /// Whether the wallpaper under the cursor is the one standby shows now.
    #[must_use]
    pub fn selected_is_current(&self) -> bool {
        match (self.selected_name(), self.current.as_deref()) {
            (Some(selected), Some(current)) => selected.eq_ignore_ascii_case(current),
            _ => false,
        }
    }

    /// Move the cursor to the next (`forward`) or previous wallpaper, going
    /// round at the ends.
    pub fn step(&mut self, forward: bool) {
        let count = self.names.len();
        if count == 0 {
            return;
        }
        self.selected = if forward {
            (self.selected + 1) % count
        } else {
            self.selected.checked_sub(1).unwrap_or(count - 1)
        };
    }

    /// Choose the wallpaper under the cursor. Only one that is on screen can
    /// be chosen; returns whether it was.
    pub fn choose(&mut self) -> bool {
        if self.preview().is_none() {
            return false;
        }
        let Some(name) = self.selected_name().map(str::to_string) else {
            return false;
        };
        self.current = Some(name.clone());
        self.chosen = Some(name);
        true
    }

    /// The choice the runtime has to store, once.
    pub fn take_choice(&mut self) -> Option<String> {
        self.chosen.take()
    }
}

#[cfg(test)]
mod tests {
    use super::SleepPickerState;
    use crate::framebuffer::FrameBuffer;

    fn listed(current: Option<&str>) -> SleepPickerState {
        let mut picker = SleepPickerState::default();
        picker.set_listing(
            vec![
                "SLEEP001.BMP".into(),
                "SLEEP002.BMP".into(),
                "SLEEP003.BMP".into(),
            ],
            current.map(str::to_string),
        );
        picker
    }

    #[test]
    fn the_cursor_starts_on_the_wallpaper_standby_shows_now() {
        let mut picker = SleepPickerState::default();
        assert!(picker.needs_listing());
        assert!(!picker.is_empty());
        picker = listed(Some("sleep002.bmp"));
        assert!(!picker.needs_listing());
        assert_eq!(picker.selected_name(), Some("SLEEP002.BMP"));
        assert_eq!(picker.position(), Some((2, 3)));
        assert!(picker.selected_is_current());
        // One that is no longer on the card: the first.
        assert_eq!(listed(Some("GONE.BMP")).position(), Some((1, 3)));
        assert_eq!(listed(None).position(), Some((1, 3)));
    }

    #[test]
    fn the_cursor_goes_round_and_asks_for_each_wallpaper_once() {
        let mut picker = listed(None);
        assert_eq!(picker.wanted_preview(), Some("SLEEP001.BMP"));
        picker.set_preview("SLEEP001.BMP", Some(FrameBuffer::new_white()));
        assert_eq!(picker.wanted_preview(), None);
        assert!(picker.preview().is_some());
        // Painted for the first time, then the same one again.
        assert!(picker.take_preview_changed());
        assert!(!picker.take_preview_changed());
        picker.step(false);
        assert_eq!(picker.selected_name(), Some("SLEEP003.BMP"));
        assert!(picker.take_preview_changed());
        // The frame in memory is another wallpaper's: not shown for this one.
        assert!(picker.preview().is_none());
        assert_eq!(picker.wanted_preview(), Some("SLEEP003.BMP"));
        // One that cannot be read is not asked for again, nor chosen.
        picker.set_preview("SLEEP003.BMP", None);
        assert_eq!(picker.wanted_preview(), None);
        assert!(!picker.choose());
        picker.step(true);
        assert_eq!(picker.selected_name(), Some("SLEEP001.BMP"));
        assert_eq!(picker.wanted_preview(), Some("SLEEP001.BMP"));
    }

    #[test]
    fn a_choice_is_handed_to_the_runtime_once_and_survives_leaving() {
        let mut picker = listed(Some("SLEEP001.BMP"));
        picker.step(true);
        // Not on screen yet: nothing to choose.
        assert!(!picker.choose());
        picker.set_preview("SLEEP002.BMP", Some(FrameBuffer::new_white()));
        assert!(!picker.selected_is_current());
        assert!(picker.choose());
        assert!(picker.selected_is_current());
        picker.close();
        assert!(picker.needs_listing());
        assert!(picker.preview().is_none());
        assert_eq!(picker.take_choice().as_deref(), Some("SLEEP002.BMP"));
        assert_eq!(picker.take_choice(), None);
    }

    #[test]
    fn an_empty_folder_has_nothing_to_step_through_or_choose() {
        let mut picker = SleepPickerState::default();
        picker.set_listing(Vec::new(), None);
        assert!(picker.is_empty());
        picker.step(true);
        picker.step(false);
        assert_eq!(picker.position(), None);
        assert_eq!(picker.wanted_preview(), None);
        assert!(!picker.choose());
    }
}
