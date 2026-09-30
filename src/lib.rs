//! Reusable application modules for the Waveshare ESP32-S3 e-Paper 3.97 board.
//!
//! Hardware-independent code stays in this library so framebuffer, routing,
//! widgets and protocol helpers can be unit-tested on the host. ESP-IDF wiring
//! remains isolated in `main.rs`.

pub mod app;
pub mod audio;
pub mod audiobook;
pub mod board_services;
pub mod boot_profile;
pub mod build_info;
pub mod buttons;
pub mod clock_time_editor;
pub mod cover_cache;
pub mod date_math;
pub mod dictionary;
pub mod dns_captive_portal;
pub mod environment;
pub mod epaper;
pub mod epub;
pub mod framebuffer;
pub mod imu;
pub mod input_events;
pub mod mcu_deep_sleep;
pub mod network;
pub mod network_config;
pub mod network_saved;
pub mod network_scan;
pub mod ntp;
pub mod orientation;
pub mod ota;
pub mod panel_refresh;
pub mod power;
pub mod power_key;
pub mod power_key_menu;
pub mod power_profile;
pub mod reader;
pub mod reading_stats;
pub mod regional;
pub mod rtc;
pub mod runtime_memory;
pub mod runtime_worker;
pub mod sd_log;
pub mod shared_i2c;
pub mod sleep_cover;
pub mod sleep_images;
pub mod sleep_mode;
pub mod sleep_network;

pub mod storage;
pub mod usb_disk;
pub mod wifi_transfer;
