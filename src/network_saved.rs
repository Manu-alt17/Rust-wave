//! Read-only saved Wi-Fi network list and rotary selection for
//! Settings > Network > Saved networks. Adding a network or changing its
//! password only happens through the phone provisioning portal; this screen
//! can only view the list and forget an entry.

/// Rows shown per page, matching the scan-result list geometry it replaces.
pub const NETWORK_SAVED_PAGE_SIZE: usize = 6;

/// One saved network as shown on-device: no password, just enough to
/// recognize it and see whether it is the one currently connected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedNetworkEntry {
    pub ssid: String,
    pub connected: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSavedUiState {
    pub networks: Vec<SavedNetworkEntry>,
    selected: usize,
    page_start: usize,
    /// `true` after a first Select on an entry, awaiting a second Select to
    /// actually forget it. Moving the selection cancels the confirmation.
    pub confirming_forget: bool,
}

impl NetworkSavedUiState {
    /// Replace the list (as refreshed by the main loop from `WIFI.TXT` and
    /// the live connection state) while keeping the selection stable by SSID
    /// where possible.
    pub fn set_networks(&mut self, networks: Vec<SavedNetworkEntry>) {
        let selected_ssid = self.selected_entry().map(|entry| entry.ssid.clone());
        self.networks = networks;
        self.confirming_forget = false;
        self.selected = selected_ssid
            .and_then(|ssid| self.networks.iter().position(|entry| entry.ssid == ssid))
            .unwrap_or(0)
            .min(self.networks.len().saturating_sub(1));
        self.sync_page();
    }

    pub fn move_previous(&mut self) {
        self.confirming_forget = false;
        if self.networks.is_empty() {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.networks.len() - 1);
        self.sync_page();
    }

    pub fn move_next(&mut self) {
        self.confirming_forget = false;
        if self.networks.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.networks.len();
        self.sync_page();
    }

    fn sync_page(&mut self) {
        self.page_start = (self.selected / NETWORK_SAVED_PAGE_SIZE) * NETWORK_SAVED_PAGE_SIZE;
    }

    #[must_use]
    pub fn visible_entries(&self) -> &[SavedNetworkEntry] {
        let end = (self.page_start + NETWORK_SAVED_PAGE_SIZE).min(self.networks.len());
        &self.networks[self.page_start..end]
    }

    #[must_use]
    pub const fn selected_on_page(&self) -> usize {
        self.selected - self.page_start
    }

    #[must_use]
    pub fn selected_entry(&self) -> Option<&SavedNetworkEntry> {
        self.networks.get(self.selected)
    }

    /// Arm the "forget?" confirmation on the currently selected entry. A
    /// no-op when the list is empty.
    pub fn begin_forget_confirmation(&mut self) {
        if self.selected_entry().is_some() {
            self.confirming_forget = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NetworkSavedUiState, SavedNetworkEntry, NETWORK_SAVED_PAGE_SIZE};

    fn entry(ssid: &str, connected: bool) -> SavedNetworkEntry {
        SavedNetworkEntry {
            ssid: ssid.into(),
            connected,
        }
    }

    #[test]
    fn navigation_wraps_and_tracks_selection() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        assert_eq!(state.selected_entry(), Some(&entry("Home", true)));
        state.move_previous();
        assert_eq!(state.selected_entry(), Some(&entry("Office", false)));
        state.move_next();
        assert_eq!(state.selected_entry(), Some(&entry("Home", true)));
    }

    #[test]
    fn moving_selection_cancels_a_pending_forget_confirmation() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        state.begin_forget_confirmation();
        assert!(state.confirming_forget);
        state.move_next();
        assert!(!state.confirming_forget);
    }

    #[test]
    fn refreshing_the_list_keeps_the_selection_on_the_same_ssid() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        state.move_next();
        assert_eq!(state.selected_entry(), Some(&entry("Office", false)));
        state.set_networks(vec![entry("Home", true), entry("Office", false), entry("Travel", false)]);
        assert_eq!(state.selected_entry(), Some(&entry("Office", false)));
    }

    #[test]
    fn empty_list_navigation_is_a_no_op() {
        let mut state = NetworkSavedUiState::default();
        state.move_next();
        state.move_previous();
        state.begin_forget_confirmation();
        assert_eq!(state.selected_entry(), None);
        assert!(!state.confirming_forget);
    }

    #[test]
    fn pagination_windows_entries_at_page_boundaries() {
        let mut state = NetworkSavedUiState::default();
        let networks: Vec<SavedNetworkEntry> = (0..(NETWORK_SAVED_PAGE_SIZE + 2))
            .map(|index| entry(&format!("Net{index}"), false))
            .collect();
        state.set_networks(networks);
        assert_eq!(state.visible_entries().len(), NETWORK_SAVED_PAGE_SIZE);
        for _ in 0..NETWORK_SAVED_PAGE_SIZE {
            state.move_next();
        }
        assert_eq!(state.selected_on_page(), 0);
        assert_eq!(state.visible_entries().len(), 2);
    }
}
