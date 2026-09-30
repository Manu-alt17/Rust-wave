//! SHTC3 temperature and humidity sensor, kept asleep.
//!
//! No feature reads it any more. The sensor idles at tens of microamps
//! unless it is told to sleep, so boot wakes it once, resets it, checks its
//! ID and sends it back to sleep (0.3 uA). Command codes from the Waveshare
//! sample and the Sensirion datasheet.

use core::fmt::Debug;

use anyhow::{anyhow, bail, Result};
use embedded_hal::{delay::DelayNs, i2c::I2c};

/// SHTC3 7-bit I2C address used by the sample firmware.
pub const SHTC3_ADDRESS: u8 = 0x70;
const READ_ID: [u8; 2] = [0xEF, 0xC8];
const SOFT_RESET: [u8; 2] = [0x80, 0x5D];
const SLEEP: [u8; 2] = [0xB0, 0x98];
const WAKEUP: [u8; 2] = [0x35, 0x17];
const CRC_POLYNOMIAL: u8 = 0x31;

/// Narrow command-level SHTC3 driver.
pub struct Shtc3<I2C> {
    i2c: I2C,
}

impl<I2C> Shtc3<I2C> {
    #[must_use]
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }
}

impl<I2C> Shtc3<I2C>
where
    I2C: I2c,
    I2C::Error: Debug,
{
    /// Wake, reset and verify the sensor, then put it to sleep. Returns the
    /// raw sensor ID for the boot log.
    pub fn put_to_sleep<D: DelayNs>(&mut self, delay: &mut D) -> Result<u16> {
        self.write_command(WAKEUP)?;
        delay.delay_us(300);
        self.write_command(SOFT_RESET)?;
        delay.delay_us(300);
        let id = self.read_id();
        self.write_command(SLEEP)?;
        id
    }

    fn read_id(&mut self) -> Result<u16> {
        let mut bytes = [0_u8; 3];
        self.i2c
            .write_read(SHTC3_ADDRESS, &READ_ID, &mut bytes)
            .map_err(|error| anyhow!("SHTC3 ID read failed: {error:?}"))?;
        verify_crc(&bytes[..2], bytes[2])?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn write_command(&mut self, command: [u8; 2]) -> Result<()> {
        self.i2c.write(SHTC3_ADDRESS, &command).map_err(|error| {
            anyhow!(
                "SHTC3 command 0x{:02X}{:02X} failed: {error:?}",
                command[0],
                command[1]
            )
        })
    }
}

fn verify_crc(bytes: &[u8], expected: u8) -> Result<()> {
    let actual = crc8(bytes);
    if actual != expected {
        bail!("SHTC3 CRC mismatch: expected 0x{expected:02X}, calculated 0x{actual:02X}");
    }
    Ok(())
}

fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0xFF_u8;
    for byte in bytes {
        crc ^= *byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ CRC_POLYNOMIAL
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::{crc8, verify_crc};

    #[test]
    fn crc_matches_sensirion_reference_vector() {
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }

    #[test]
    fn rejects_id_crc_mismatch() {
        assert!(verify_crc(&[0xBE, 0xEF], 0x00).is_err());
        assert!(verify_crc(&[0xBE, 0xEF], 0x92).is_ok());
    }
}
