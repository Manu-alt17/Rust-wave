//! "Connect to PC": the microSD card as a USB disk, so books and
//! audiobooks can be copied straight from a computer.
//!
//! The USB side is a small C component over Espressif's TinyUSB stack
//! (`components/usbdisk`, see its header for the hardware constraints).
//! Disk mode is one way: it ends by restarting the device, the only way to
//! hand the shared USB PHY back to the serial port -- and it rereads the
//! library, so what was just copied shows up.

/// Where the "Connect to PC" screen stands.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum UsbDiskPhase {
    /// Explaining what is about to happen; SELECT connects.
    #[default]
    Idle,
    /// The PC sees the disk. Any key but BOOT restarts the device.
    Active,
    /// Disk mode could not start; a key restarts the device.
    Failed(String),
}

#[cfg(target_os = "espidf")]
pub mod espidf {
    use esp_idf_svc::sys::usbdisk as ffi;

    /// SDMMC pins of this board, as `components/usbdisk` wants them.
    #[derive(Clone, Copy, Debug)]
    pub struct SdPins {
        pub clk: i32,
        pub cmd: i32,
        pub d0: i32,
        pub d1: i32,
        pub d2: i32,
        pub d3: i32,
    }

    /// Hand the USB PHY back to the serial port. Called at every boot: a
    /// crash in disk mode must not leave the programming port stuck.
    pub fn release_phy() {
        unsafe { ffi::usbdisk_release_phy() };
    }

    /// Start disk mode. The SD card must already be unmounted.
    pub fn start(pins: SdPins) -> Result<(), String> {
        let status =
            unsafe { ffi::usbdisk_start(pins.clk, pins.cmd, pins.d0, pins.d1, pins.d2, pins.d3) };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("USB disk start failed (error {status})"))
        }
    }

    /// Leave disk mode: give the PHY back, then restart.
    pub fn restart() -> ! {
        unsafe { ffi::usbdisk_restart() };
        #[allow(unreachable_code)]
        loop {}
    }
}
