//! Native panel refresh coordinator.
//!
//! All e-paper mode decisions remain Rust-owned. SD-loaded applications may
//! submit dirty rectangles and draw intent, but they never select SSD1677
//! commands directly. The coordinator deliberately retains the proven
//! full-screen partial transport until windowed RAM writes receive their own
//! isolated hardware experiment.

/// Periodic ghost-cleanup cadence shared by menus, Reader screens and games.
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
    /// keep the full refresh. A full-page image transition is the same
    /// ghost cleanup as the periodic one, just triggered by content.
    #[must_use]
    pub const fn uses_fast_waveform(self) -> bool {
        matches!(
            self,
            Self::AfterWake | Self::PeriodicCleanup | Self::FullPageImageTransition
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRefreshPlan {
    PartialFullscreen { partial_count: u8 },
    GlobalBase { reason: PanelGlobalReason },
}

/// One counter for every normal UI and game refresh. This replaces the former
/// split between the six-refresh UI counter and the independent game policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PanelRefreshCoordinator {
    partial_count: u8,
    /// Whether the last frame planned showed a full-page image.
    showing_full_page_image: bool,
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
        self.plan_for_frame(request, self.showing_full_page_image)
    }

    /// Like [`Self::plan`], for a frame that does (`full_page_image`) or does
    /// not show a full-page image. Entering or leaving such a frame turns a
    /// normal refresh into a fast global one, exactly like the periodic
    /// cleanup, and restarts the partial counter.
    #[must_use]
    pub fn plan_for_frame(
        &mut self,
        request: PanelRefreshRequest,
        full_page_image: bool,
    ) -> PanelRefreshPlan {
        let image_transition = full_page_image != self.showing_full_page_image;
        self.showing_full_page_image = full_page_image;
        if image_transition && request == PanelRefreshRequest::Normal {
            self.partial_count = 0;
            return PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::FullPageImageTransition,
            };
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

#[cfg(test)]
mod tests {
    use super::{
        PanelGlobalReason, PanelRefreshCoordinator, PanelRefreshPlan, PanelRefreshRequest,
        PANEL_PARTIAL_REFRESH_LIMIT,
    };

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
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, false),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
        // Entering the cover.
        assert_eq!(coordinator.plan_for_frame(PanelRefreshRequest::Normal, true), transition);
        assert_eq!(coordinator.partial_count(), 0);
        // Redrawing the cover itself (clock tick, overlay) stays partial.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, true),
            PanelRefreshPlan::PartialFullscreen { partial_count: 1 }
        );
        // Leaving the cover.
        assert_eq!(coordinator.plan_for_frame(PanelRefreshRequest::Normal, false), transition);
        assert!(PanelGlobalReason::FullPageImageTransition.uses_fast_waveform());
    }

    #[test]
    fn a_forced_global_on_an_image_transition_keeps_its_own_reason() {
        let mut coordinator = PanelRefreshCoordinator::default();
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::AfterWake, true),
            PanelRefreshPlan::GlobalBase {
                reason: PanelGlobalReason::AfterWake
            }
        );
        // The transition was already covered by that global refresh.
        assert_eq!(
            coordinator.plan_for_frame(PanelRefreshRequest::Normal, true),
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
