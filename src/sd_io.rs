//! Reading files from the microSD at full speed on the ESP32-S3.
//!
//! The SDMMC driver can DMA only into internal RAM. When the destination is
//! in PSRAM, where every allocation over 16 KB lands
//! (`CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL`), it reads one 512-byte sector per
//! command through a bounce buffer of its own. Measured on the board, 4 MB
//! read into a 64 KB PSRAM buffer took 4.0 s, and 0.89 s through an 8 KB
//! internal one. Reads into PSRAM therefore go through an internal buffer of
//! [`BOUNCE_BYTES`] here, so the card still gets multi-sector transfers.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

/// Size of the internal buffer large reads go through: already enough for
/// multi-sector transfers, and well under the 16 KB above which it would
/// land in PSRAM itself.
const BOUNCE_BYTES: usize = 8 * 1024;

/// Fill `buffer` from `reader`, stopping early only at the end of the file.
/// Returns the number of bytes read.
pub fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    if buffer.len() <= BOUNCE_BYTES || !is_external(buffer) {
        return read_direct(reader, buffer);
    }
    let mut bounce = vec![0_u8; BOUNCE_BYTES];
    let mut filled = 0;
    while filled < buffer.len() {
        let want = (buffer.len() - filled).min(BOUNCE_BYTES);
        let read = read_direct(reader, &mut bounce[..want])?;
        buffer[filled..filled + read].copy_from_slice(&bounce[..read]);
        filled += read;
        if read < want {
            break;
        }
    }
    Ok(filled)
}

/// `std::fs::read`, through [`read_full`].
pub fn read_file(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let length = usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "file too large"))?;
    let mut bytes = vec![0_u8; length];
    let read = read_full(&mut file, &mut bytes)?;
    bytes.truncate(read);
    Ok(bytes)
}

fn read_direct(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

#[cfg(target_os = "espidf")]
fn is_external(buffer: &[u8]) -> bool {
    unsafe { esp_idf_svc::sys::esp_ptr_external_ram(buffer.as_ptr().cast()) }
}

/// On the host every buffer takes the bounce path once it is large enough,
/// so the tests exercise it.
#[cfg(not(target_os = "espidf"))]
fn is_external(_buffer: &[u8]) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use super::{read_file, read_full, BOUNCE_BYTES};

    /// Hands out at most `step` bytes per call, like a slow device.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.step.min(buffer.len()).min(self.data.len());
            buffer[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    #[test]
    fn large_reads_arrive_whole_and_in_order() {
        let data: Vec<u8> = (0..BOUNCE_BYTES * 3 + 123).map(|i| (i * 7 % 251) as u8).collect();
        for step in [1_000, BOUNCE_BYTES, 100_000] {
            let mut buffer = vec![0_u8; data.len()];
            let read = read_full(&mut Trickle { data: &data, step }, &mut buffer).unwrap();
            assert_eq!(read, data.len());
            assert_eq!(buffer, data);
            // A short file stops at its end.
            let mut longer = vec![0_u8; data.len() + 5_000];
            let read = read_full(&mut Trickle { data: &data, step }, &mut longer).unwrap();
            assert_eq!(read, data.len());
            assert_eq!(&longer[..read], &data[..]);
        }
    }

    #[test]
    fn read_file_matches_std() {
        let path = std::env::temp_dir().join(format!("rustmix-sd-io-{}", std::process::id()));
        let data: Vec<u8> = (0..50_000).map(|i| (i % 253) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        assert_eq!(read_file(&path).unwrap(), data);
        std::fs::remove_file(&path).unwrap();
    }
}
