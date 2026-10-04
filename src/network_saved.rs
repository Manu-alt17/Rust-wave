//! Saved Wi-Fi network list, rotary selection and per-network actions for
//! Settings > Network > Saved networks. Adding a network or changing its
//! password only happens through the phone provisioning portal; this screen
//! lists the saved ones and can connect to one or forget it.

use crate::regional::Locale;

/// Rows shown per page.
pub const NETWORK_SAVED_PAGE_SIZE: usize = 6;

/// One saved network as shown on-device: no password, just enough to
/// recognize it and see whether it is the one currently connected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedNetworkEntry {
    pub ssid: String,
    pub connected: bool,
}

/// What SELECT on a saved network offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavedNetworkAction {
    /// Drop the current connection and join this network.
    Connect,
    /// Remove it from `WIFI.TXT`.
    Forget,
    /// Close the menu.
    Cancel,
}

impl SavedNetworkAction {
    #[must_use]
    pub const fn label_i18n(self, locale: Locale) -> &'static str {
        match (self, locale) {
            (Self::Connect, Locale::English) => "Connect",
            (Self::Connect, Locale::Italian) => "Connetti",
            (Self::Forget, Locale::English) => "Forget this network",
            (Self::Forget, Locale::Italian) => "Dimentica questa rete",
            (Self::Cancel, Locale::English) => "Cancel",
            (Self::Cancel, Locale::Italian) => "Annulla",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSavedUiState {
    pub networks: Vec<SavedNetworkEntry>,
    selected: usize,
    page_start: usize,
    /// Selected row of the action menu while it is open on the selected
    /// network; `None` while the list itself is shown. Moving to another
    /// network is not possible with the menu open, and a refreshed list
    /// closes it.
    pub menu: Option<usize>,
}

impl NetworkSavedUiState {
    /// Replace the list (as refreshed by the main loop from `WIFI.TXT` and
    /// the live connection state) while keeping the selection stable by SSID
    /// where possible.
    pub fn set_networks(&mut self, networks: Vec<SavedNetworkEntry>) {
        let selected_ssid = self.selected_entry().map(|entry| entry.ssid.clone());
        let unchanged = self.networks == networks;
        self.networks = networks;
        if !unchanged {
            self.menu = None;
        }
        self.selected = selected_ssid
            .and_then(|ssid| self.networks.iter().position(|entry| entry.ssid == ssid))
            .unwrap_or(0)
            .min(self.networks.len().saturating_sub(1));
        self.sync_page();
    }

    /// Mark the network the device is connected to (`None` while it is not
    /// connected), from the live connection state. A menu left open on a
    /// network whose state just changed is closed: its actions changed.
    pub fn mark_connected(&mut self, ssid: Option<&str>) {
        let mut changed = false;
        for entry in &mut self.networks {
            let connected = ssid == Some(entry.ssid.as_str());
            changed |= entry.connected != connected;
            entry.connected = connected;
        }
        if changed {
            self.menu = None;
        }
    }

    pub fn move_previous(&mut self) {
        if self.networks.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.networks.len() - 1);
        self.sync_page();
    }

    pub fn move_next(&mut self) {
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

    /// `(page from 1, pages)` for the footer.
    #[must_use]
    pub fn page_position(&self) -> (usize, usize) {
        (
            self.page_start / NETWORK_SAVED_PAGE_SIZE + 1,
            self.networks.len().max(1).div_ceil(NETWORK_SAVED_PAGE_SIZE),
        )
    }

    #[must_use]
    pub fn selected_entry(&self) -> Option<&SavedNetworkEntry> {
        self.networks.get(self.selected)
    }

    /// Actions the menu offers for the selected network: no "Connect" for
    /// the one already connected.
    #[must_use]
    pub fn menu_actions(&self) -> Vec<SavedNetworkAction> {
        match self.selected_entry() {
            Some(entry) if entry.connected => {
                vec![SavedNetworkAction::Forget, SavedNetworkAction::Cancel]
            }
            Some(_) => vec![
                SavedNetworkAction::Connect,
                SavedNetworkAction::Forget,
                SavedNetworkAction::Cancel,
            ],
            None => Vec::new(),
        }
    }

    /// Open the action menu on the selected network. A no-op when the list
    /// is empty.
    pub fn open_menu(&mut self) {
        if self.selected_entry().is_some() {
            self.menu = Some(0);
        }
    }

    /// Close the action menu; `false` when it was not open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    pub fn menu_previous(&mut self) {
        let count = self.menu_actions().len();
        if let Some(selected) = self.menu.as_mut() {
            if count > 0 {
                *selected = selected.checked_sub(1).unwrap_or(count - 1);
            }
        }
    }

    pub fn menu_next(&mut self) {
        let count = self.menu_actions().len();
        if let Some(selected) = self.menu.as_mut() {
            if count > 0 {
                *selected = (*selected + 1) % count;
            }
        }
    }

    /// The action under the menu cursor, when the menu is open.
    #[must_use]
    pub fn selected_menu_action(&self) -> Option<SavedNetworkAction> {
        self.menu
            .and_then(|selected| self.menu_actions().get(selected).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NetworkSavedUiState, SavedNetworkAction, SavedNetworkEntry, NETWORK_SAVED_PAGE_SIZE,
    };

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
    fn the_connected_network_follows_the_live_connection() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Casa", false), entry("Ufficio", true)]);
        state.open_menu();
        state.mark_connected(Some("Casa"));
        assert!(state.networks[0].connected);
        assert!(!state.networks[1].connected);
        // The open menu's actions changed, so it closed.
        assert!(state.menu.is_none());
        state.open_menu();
        state.mark_connected(Some("Casa"));
        assert!(state.menu.is_some());
        state.mark_connected(None);
        assert!(state.networks.iter().all(|entry| !entry.connected));
    }

    #[test]
    fn the_menu_offers_connect_only_for_a_network_not_connected() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        state.open_menu();
        assert_eq!(
            state.menu_actions(),
            vec![SavedNetworkAction::Forget, SavedNetworkAction::Cancel]
        );
        assert!(state.close_menu());
        state.move_next();
        state.open_menu();
        assert_eq!(
            state.selected_menu_action(),
            Some(SavedNetworkAction::Connect)
        );
        state.menu_next();
        assert_eq!(
            state.selected_menu_action(),
            Some(SavedNetworkAction::Forget)
        );
        state.menu_previous();
        state.menu_previous();
        assert_eq!(
            state.selected_menu_action(),
            Some(SavedNetworkAction::Cancel)
        );
    }

    #[test]
    fn a_changed_list_closes_the_menu_and_an_unchanged_one_keeps_it() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        state.open_menu();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        assert!(state.menu.is_some());
        state.set_networks(vec![entry("Home", true)]);
        assert!(state.menu.is_none());
    }

    #[test]
    fn refreshing_the_list_keeps_the_selection_on_the_same_ssid() {
        let mut state = NetworkSavedUiState::default();
        state.set_networks(vec![entry("Home", true), entry("Office", false)]);
        state.move_next();
        assert_eq!(state.selected_entry(), Some(&entry("Office", false)));
        state.set_networks(vec![
            entry("Home", true),
            entry("Office", false),
            entry("Travel", false),
        ]);
        assert_eq!(state.selected_entry(), Some(&entry("Office", false)));
    }

    #[test]
    fn empty_list_navigation_is_a_no_op() {
        let mut state = NetworkSavedUiState::default();
        state.move_next();
        state.move_previous();
        state.open_menu();
        assert_eq!(state.selected_entry(), None);
        assert!(state.menu.is_none());
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
