//! Native panel refresh coordinator.
//!
//! All e-paper mode decisions remain Rust-owned. SD-loaded applications may
//! submit dirty rectangles and draw intent, but they never select SSD1677
//! commands directly. The coordinator deliberately retains the proven
//! full-screen partial transport until windowed RAM writes receive their own
//! isolated hardware experiment.

/// Periodic ghost-cleanup cadence shared by menus and Reader screens.
///
/// Waveshare's e-paper FAQ and GoodDisplay's panel documentation call for a
/// full refresh after at most 5 consecutive partial/fast refreshes to keep
/// ghosting in check, but that's the conservative floor, not a hard limit.
/// Now that the periodic cleanup uses the single-flash fast waveform
/// ([`crate::epaper::Epaper397::show_base_fast`]) instead of the old
/// multi-flash one, 50 trades a wider ghosting margin (to be validated by
/// field testing on the actual panel) for noticeably less disruption than
/// either the vendor floor or this project's previous value of 24.
pub const PANEL_PARTIAL_REFRESH_LIMIT: u8 = 50;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRefreshRequest {
    Normal,
    AfterWake,
    ManualGhostCleanup,
    SafetyFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelGlobalReason {
    InitialBoot,
    AfterWake,
    ManualGhostCleanup,
    PeriodicCleanup,
    SafetyFallback,
    SleepImage,
    /// The screen switches to or from a full-page image (an EPUB cover or
    /// plate). A dithered full-page bitmap leaves far heavier ghosting than
    /// text, both on the image when it appears over a text page and on the
    /// text page drawn over it afterwards.
    FullPageImageTransition,
    /// The screen leaves an inverted (white-on-black, High Contrast) Reader
    /// page. A partial refresh from a mostly black frame back to the white
    /// UI leaves the whole page ghosted behind it.
    InvertedPageExit,
}

impl PanelGlobalReason {
    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::InitialBoot => "initial-boot",
            Self::AfterWake => "after-wake",
            Self::ManualGhostCleanup => "manual-ghost-cleanup",
            Self::PeriodicCleanup => "ghost-cleanup-threshold",
            Self::SafetyFallback => "safety-fallback",
            Self::SleepImage => "sleep-image",
            Self::FullPageImageTransition => "full-page-image-transition",
            Self::InvertedPageExit => "inverted-page-exit",
        }
    }

    /// Whether this global refresh should use the fast waveform (0x22 0xD7)
    /// instead of the standard one (0x22 0xF7).
    ///
    /// The periodic ghost-cleanup cycle and waking the panel back up both
    /// happen routinely during normal use — `AfterWake` fires every time the
    /// panel comes back from `PANEL_IDLE_SLEEP_SECONDS` (60s) of idle, which
    /// is far more often than an actual hardware deep sleep — so both are
    /// worth trading the standard waveform's sensor-read accuracy for a
    /// single flash. Initial boot, manual cleanup, the safety fallback and
    /// the sleep image are rare or deliberately cautious events, so they
    /// keep the full refresh. A full-page image transition and leaving an
    /// inverted page are the same ghost cleanup as the periodic one, just
    /// triggered by content.
    #[must_use]
    pub const fn uses_fast_waveform(self) -> bool {
        matches!(
            self,
            Self::AfterWake
                | Self::PeriodicCleanup
                | Self::FullPageImageTransition
                | Self::InvertedPageExit
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRefreshPlan {
    PartialFullscreen { partial_count: u8 },
    GlobalBase { reason: PanelGlobalReason },
}

/// One counter for every normal UI refresh. This replaces the former
/// split between the six-refresh UI counter and a separate game policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PanelRefreshCoordinator {
    partial_count: u8,
    /// Whether the last frame planned showed a full-page image.
    showing_full_page_image: bool,
    /// Whether the last frame planned was drawn inverted (High Contrast).
    showing_inverted: bool,
}

impl PanelRefreshCoordinator {
    #[must_use]
    pub const fn partial_count(self) -> u8 {
        self.partial_count
    }

    pub fn reset_after_external_global(&mut self, _reason: PanelGlobalReason) {
        self.partial_count = 0;
    }

    #[must_use]
    pub fn plan(&mut self, request: PanelRefreshRequest) -> PanelRefreshPlan {
        self.plan_for_frame(request, self.showing_full_page_image, self.showing_inverted)
    }

    /// Like [`Self::plan`], for a frame that does (`full_page_image`) or does
    /// not show a full-page image, and is (`inverted`) or is not drawn
    /// inverted. Entering or leaving a full-page image, or leaving an
    /// inverted frame, turns a normal refresh into a fast global one,
    /// exactly like the periodic cleanup, and restarts the partial counter.
    #[must_use]
    pub fn plan_for_frame(
        &mut self,
        request: PanelRefreshRequest,
        full_page_image: bool,
        inverted: bool,
    ) -> PanelRefreshPlan {
        let image_transition = full_page_image != self.showing_full_page_image;
        let inverted_exit = self.showing_inverted && !inverted;
        self.showing_full_page_image = full_page_image;
        self.showing_inverted = inverted;
        if request == PanelRefreshRequest::Normal {
            let reason = if inverted_exit {
                Some(PanelGlobalReason::InvertedPageExit)
            } else if image_transition {
                Some(PanelGlobalReason::FullPageImageTransition)
            } else {
                None
            };
            if let Some(reason) = reason {
                self.partial_count = 0;
                return PanelRefreshPlan::GlobalBase { reason };
            }
        }
        let forced = match request {
            PanelRefreshRequest::Normal => None,
            PanelRefreshRequest::AfterWake => Some(PanelGlobalReason::AfterWake),
            PanelRefreshRequest::ManualGhostCleanup => Some(PanelGlobalReason::ManualGhostCleanup),
            PanelRefreshRequest::SafetyFallback => Some(PanelGlobalReason::SafetyFallback),
        };
        if let Some(reason) = forced {
            self.partial_count = 0;
            return PanelRefreshPlan::GlobalBase { reason };
        }
        if self.partial_count >= PANEL_PARTIAL_REFRESH_LIMIT {
            self.partial_count = 0;
            return PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::PeriodicCleanup,
            };
        }
        self.partial_count = self.partial_count.saturating_add(1);
        PanelRefreshPlan::PartialFullscreen {
            partial_count: self.partial_count,
        }
    }
}

/// Longest time powered off after which the wake-from-sleep refresh may use
/// the fast waveform instead of the full one.
///
/// Sleep entry always draws the sleep image with the full waveform, so the
/// panel gets a complete (DC-balanced, ghost-clearing) cycle every time it
/// is locked. On unlock the fast waveform is enough to replace that freshly
/// written image -- unless it has been sitting on the glass long enough to
/// be retained more strongly, in which case the full waveform is used again.
pub const FAST_WAKE_MAX_SLEEP_SECONDS: u64 = 12 * 60 * 60;

/// Whether the refresh that ends a wake from sleep may use the fast
/// waveform, given the UTC unix time recorded at sleep entry and the one at
/// wake. Anything unknown or inconsistent (no record, no clock, a clock that
/// went backwards) falls back to the full waveform.
#[must_use]
pub fn wake_uses_fast_waveform(slept_at: Option<u64>, now: Option<u64>) -> bool {
    matches!(
        (slept_at, now),
        (Some(slept_at), Some(now))
            if now >= slept_at && now - slept_at < FAST_WAKE_MAX_SLEEP_SECONDS
    )
}

/// Parse the sleep-entry timestamp file written at sleep entry: a single
/// decimal UTC unix time, surrounding whitespace ignored.
#[must_use]
pub fn parse_sleep_timestamp(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{
        parse_sleep_timestamp, wake_uses_fast_waveform, PanelGlobalReason,
        PanelRefreshCoordinator, PanelRefreshPlan, PanelRefreshRequest,
        FAST_WAKE_MAX_SLEEP_SECONDS, PANEL_PARTIAL_REFRESH_LIMIT,
    };

    #[test]
    fn fast_wake_only_for_known_short_sleeps() {
        let slept = 1_800_000_000;
        assert!(wake_uses_fast_waveform(Some(slept), Some(slept + 60)));
        assert!(wake_uses_fast_waveform(
            Some(slept),
            Some(slept + FAST_WAKE_MAX_SLEEP_SECONDS - 1)
        ));
        assert!(!wake_uses_fast_waveform(
            Some(slept),
            Some(slept + FAST_WAKE_MAX_SLEEP_SECONDS)
        ));
        assert!(!wake_uses_fast_waveform(Some(slept), Some(slept - 1)), "clock went back");
        assert!(!wake_uses_fast_waveform(None, Some(slept)), "no sleep record");
        assert!(!wake_uses_fast_waveform(Some(slept), None), "no clock");
    }

    #[test]
    fn sleep_timestamp_parses_one_trimmed_number() {
        assert_eq!(parse_sleep_timestamp("1800000000\n"), Some(1_800_000_000));
        assert_eq!(parse_sleep_timestamp(" 42 "), Some(42));
        assert_eq!(parse_sleep_timestamp(""), None);
        assert_eq!(parse_sleep_timestamp("18000x"), None);
    }

    #[test]
    fn normal_routes_share_one_partial_counter_before_periodic_cleanup() {
        let mut coordinator = PanelRefreshCoordinator::default();
        for partial_count in 1..=PANEL_PARTIAL_REFRESH_LIMIT {
            assert_eq!(
                coordinator.plan(PanelRefreshRequest::Normal),
                PanelRefreshPlan::PartialFullscreen { partial_count }
            );
        }
        assert_eq!(
            coordinator.plan(PanelRefreshRequest::Normal),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::PeriodicCleanup
            }
        );
    }

    #[test]
    fn wake_manual_and_safety_requests_force_global_base() {
        let mut coordinator = PanelRefreshCoordinator::default();
        let _ = coordinator.plan(PanelRefreshRequest::Normal);
        assert_eq!(
            coordinator.plan(PanelRefreshRequest::AfterWake),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::AfterWake
            }
        );
        assert_eq!(coordinator.partial_count(), 0);
        assert_eq!(
            coordinator.plan(PanelRefreshRequest::ManualGhostCleanup),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::ManualGhostCleanup
            }
        );
        assert_eq!(
            coordinator.plan(PanelRefreshRequest::SafetyFallback),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::SafetyFallback
            }
        );
    }

    #[test]
    fn entering_and_leaving_a_full_page_image_forces_a_fast_global_refresh() {
        let mut coordinator = PanelRefreshCoordinator::default();
        let transition = PanelRefreshPlan::GlobalBase {
            reason: PanelGlobalReason::FullPageImageTransition,
        };
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, false),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
        // Entering the cover.
        assert_eq!(coordinator.plan_for_frame(PanelRefreshRequest::Normal, true, false), transition);
        assert_eq!(coordinator.partial_count(), 0);
        // Redrawing the cover itself (clock tick, overlay) stays partial.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, true, false),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
        // Leaving the cover.
        assert_eq!(coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, false), transition);
        assert!(PanelGlobalReason::FullPageImageTransition.uses_fast_waveform());
    }

    #[test]
    fn a_forced_global_on_an_image_transition_keeps_its_own_reason() {
        let mut coordinator = PanelRefreshCoordinator::default();
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::AfterWake, true, false),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::AfterWake
            }
        );
        // The transition was already covered by that global refresh.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, true, false),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
    }

    #[test]
    fn leaving_an_inverted_page_forces_a_fast_global_refresh() {
        let mut coordinator = PanelRefreshCoordinator::default();
        // Entering and turning inverted pages stays partial.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, true),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, true),
            PanelRefreshPlan::PartialFullscreen { partial_count: 2 }
        );
        // Leaving for the normal white UI.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, false),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::InvertedPageExit
            }
        );
        assert_eq!(coordinator.partial_count(), 0);
        assert!(PanelGlobalReason::InvertedPageExit.uses_fast_waveform());
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false, false),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
    }

    #[test]
    fn periodic_cleanup_and_wake_use_the_fast_waveform() {
        assert!(PanelGlobalReason::PeriodicCleanup.uses_fast_waveform());
        assert!(PanelGlobalReason::AfterWake.uses_fast_waveform());
        assert!(!PanelGlobalReason::InitialBoot.uses_fast_waveform());
        assert!(!PanelGlobalReason::ManualGhostCleanup.uses_fast_waveform());
        assert!(!PanelGlobalReason::SafetyFallback.uses_fast_waveform());
        assert!(!PanelGlobalReason::SleepImage.uses_fast_waveform());
    }
}
