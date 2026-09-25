//! Product identity and semantic firmware metadata.
//!
//! Keep product-facing metadata in one place so UI screens and serial markers
//! report the same values. The ESP-IDF application version is also pinned in
//! `sdkconfig.defaults` for the bootloader application descriptor.

/// Human-readable product label rendered in the UI.
pub const PRODUCT_NAME: &str = "Rustmix Wave / EPD397";
/// Stable machine-readable product identifier used in serial markers.
pub const PRODUCT_SLUG: &str = "rustmix-wave-epd397";
/// Cargo semantic version for the current firmware package.
pub const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Stable milestone identifier for acceptance logs and diagnostics.
pub const UI_SHELL_MILESTONE: &str = "text-editor-layout-alignment";

/// GitHub repository owner checked by [`crate::ota`] for release-based OTA
/// updates.
///
/// TEMPORARY: pointed at the `Manu-alt17/Rust-wave` test fork (must be
/// public -- the unauthenticated GitHub release API 404s on private repos)
/// while validating the OTA flow end to end. Repoint at `aimindseye` once
/// push access to the upstream repo is available and the flow is verified.
pub const OTA_REPO_OWNER: &str = "Manu-alt17";
/// GitHub repository name checked by [`crate::ota`] for release-based OTA
/// updates. The latest release there must publish a `.bin` asset built from
/// this same firmware (see `scripts/build-release-firmware.sh`, which emits
/// one alongside the ELF).
pub const OTA_REPO_NAME: &str = "Rust-wave";

#[cfg(test)]
mod tests {
    use super::{FIRMWARE_VERSION, PRODUCT_NAME, PRODUCT_SLUG, UI_SHELL_MILESTONE};

    #[test]
    fn exposes_text_editor_layout_alignment_metadata() {
        assert_eq!(PRODUCT_NAME, "Rustmix Wave / EPD397");
        assert_eq!(PRODUCT_SLUG, "rustmix-wave-epd397");
        assert_eq!(FIRMWARE_VERSION, env!("CARGO_PKG_VERSION"));
        assert_eq!(UI_SHELL_MILESTONE, "text-editor-layout-alignment");
    }
}
