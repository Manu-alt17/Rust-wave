#pragma once

/* The microSD card as a USB mass-storage disk for a PC.
 *
 * Kept in C, as an ESP-IDF component the Rust firmware calls through
 * bindings (see Cargo.toml's extra_components): the USB descriptors are
 * TinyUSB macros, and this follows ESP-IDF's
 * examples/peripherals/usb/device/tusb_msc closely.
 *
 * One way only: disk mode ends by restarting the device. On the ESP32-S3
 * the native USB PHY is shared between USB-Serial-JTAG (the programming
 * and log port) and USB-OTG (the disk); while the PC sees the disk, the
 * serial port does not exist. Restarting is the reliable way to give the
 * PHY back, and it rereads the library, so books just copied show up.
 *
 * For the same reason it is only ever started on an explicit request from
 * the user, never at boot: a fault in this mode can never take the
 * programming port away for good.
 *
 * Before calling it the SD card must be unmounted and every file on it
 * closed: the firmware and the PC cannot use one filesystem at once. */

#include <stdint.h>

#include "esp_err.h"

/* Start disk mode on the SDMMC 4-bit bus with these pins. */
esp_err_t usbdisk_start(int clk, int cmd, int d0, int d1, int d2, int d3);

/* Give the USB PHY back to the serial port, setting the two RTC bits that
 * choose it (RTC_CNTL_SW_HW_USB_PHY_SEL, RTC_CNTL_SW_USB_PHY_SEL) back to
 * their power-on value. TinyUSB switches them to "disk" and nothing ever
 * switches them back, and RTC registers survive a software restart: after
 * a plain esp_restart() the PHY stayed wired to a peripheral nobody drove,
 * with neither serial port nor disk, until power was removed -- which a
 * connected battery never does. Also called at every boot, so a crash in
 * disk mode cannot leave the port stuck either. */
void usbdisk_release_phy(void);

/* Leave disk mode: release the PHY, then restart. */
void usbdisk_restart(void) __attribute__((noreturn));
