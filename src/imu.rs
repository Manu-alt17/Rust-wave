//! QMI8658 six-axis IMU, kept powered down.
//!
//! No feature reads motion any more. The chip sits on the always-on VCC3V3
//! rail (it is not switched by an AXP2101 channel like the e-paper panel),
//! and an earlier firmware may have left its 1000 Hz accelerometer and
//! gyroscope running with the tap engine on. Boot therefore finds it once
//! and switches all of that off. Register offsets from QST's QMI8658 map as
//! used by the Waveshare sample and lewisxhe/SensorLib.

use core::fmt::Debug;

use anyhow::{anyhow, bail, Result};
use embedded_hal::i2c::I2c;

/// QMI8658 SA0-low address used by the uploaded Waveshare sample.
pub const QMI8658_ADDRESS_LOW: u8 = 0x6A;
/// QMI8658 SA0-high fallback address supported by the uploaded sample.
pub const QMI8658_ADDRESS_HIGH: u8 = 0x6B;
/// Device identifier required by the sample driver's probe loop.
pub const QMI8658_WHO_AM_I_VALUE: u8 = 0x05;

const WHO_AM_I: u8 = 0x00;
const REVISION: u8 = 0x01;
/// CTRL7 bit 0 enables the accelerometer, bit 1 the gyroscope.
const CTRL7: u8 = 0x08;
/// CTRL8 holds the tap, any/no-motion and pedometer engine enables.
const CTRL8: u8 = 0x09;

/// Where the chip was found, for the boot log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImuPowerDownReport {
    pub address: u8,
    pub revision: u8,
}

/// Narrow register-level QMI8658 driver.
pub struct Qmi8658<I2C> {
    i2c: I2C,
}

impl<I2C> Qmi8658<I2C> {
    #[must_use]
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }
}

impl<I2C> Qmi8658<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    /// Probe both SA0 addresses, then disable the motion engines and both
    /// sensors, and check that the sensors really stopped.
    pub fn power_down(&mut self) -> Result<ImuPowerDownReport> {
        let address = self.probe_address()?;
        let revision = self.read_register(address, REVISION)?;
        self.write_register(address, CTRL8, 0x00)?;
        self.write_register(address, CTRL7, 0x00)?;
        let enabled = self.read_register(address, CTRL7)?;
        if enabled & 0x03 != 0 {
            bail!("QMI8658 CTRL7 still enables sensors after power-down: 0x{enabled:02X}");
        }
        Ok(ImuPowerDownReport { address, revision })
    }

    fn probe_address(&mut self) -> Result<u8> {
        for address in [QMI8658_ADDRESS_LOW, QMI8658_ADDRESS_HIGH] {
            let mut who_am_i = [0_u8; 1];
            if self
                .i2c
                .write_read(address, &[WHO_AM_I], &mut who_am_i)
                .is_ok()
                && who_am_i[0] == QMI8658_WHO_AM_I_VALUE
            {
                return Ok(address);
            }
        }
        bail!(
            "QMI8658 probe failed: expected WHO_AM_I 0x{QMI8658_WHO_AM_I_VALUE:02X} at 0x{QMI8658_ADDRESS_LOW:02X} or 0x{QMI8658_ADDRESS_HIGH:02X}"
        )
    }

    fn read_register(&mut self, address: u8, register: u8) -> Result<u8> {
        let mut value = [0_u8; 1];
        self.i2c
            .write_read(address, &[register], &mut value)
            .map_err(|error| anyhow!("QMI8658 read 0x{register:02X} failed: {error:?}"))?;
        Ok(value[0])
    }

    fn write_register(&mut self, address: u8, register: u8, value: u8) -> Result<()> {
        self.i2c
            .write(address, &[register, value])
            .map_err(|error| anyhow!("QMI8658 write 0x{register:02X} failed: {error:?}"))
    }
}
