//! Reading files from the microSD at full speed on the ESP32-S3.
//!
//! FatFs hands whole sectors to the SDMMC driver straight into the caller's
//! buffer, and the driver can DMA only into internal RAM at a word-aligned
//! address. Anything else -- a destination in PSRAM, where every allocation
//! over 16 KB lands (`CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL`), or one shifted
//! by a read that started mid-sector -- it reads one 512-byte sector per
//! command through a bounce buffer of its own. Measured on the board: 4 MB
//! into a 64 KB PSRAM buffer took 4.0 s, and 0.89 s through an 8 KB internal
//! one; a 1.46 MB EPUB member at an odd offset took 3.8 s, 0.8 s once
//! aligned. So larger reads first go up to the next sector boundary, then
//! through an internal buffer of [`BOUNCE_BYTES`], and the card gets
//! multi-sector transfers.

use std::{
    fs::File,
    io::{self, Read, Seek},
    path::Path,
};

/// Size of the internal buffer large reads go through: already enough for
/// multi-sector transfers, and well under the 16 KB above which it would
/// land in PSRAM itself.
const BOUNCE_BYTES: usize = 8 * 1024;

/// FAT sector size on the card.
const SECTOR_BYTES: u64 = 512;

/// Reads shorter than this go straight to FatFs: they hold at most one
/// whole sector, which costs one command whichever way it is read.
const DIRECT_READ_BYTES: usize = 2 * SECTOR_BYTES as usize;

/// Fill `buffer` from `reader`, stopping early only at the end of the file.
/// Returns the number of bytes read.
pub fn read_full<R: Read + Seek>(reader: &mut R, buffer: &mut [u8]) -> io::Result<usize> {
    if buffer.len() < DIRECT_READ_BYTES {
        return read_direct(reader, buffer);
    }
    // Up to the sector boundary first: FatFs serves it from its own sector
    // buffer, and every read after it starts on a whole sector.
    let position = reader.stream_position()?;
    let lead = ((SECTOR_BYTES - position % SECTOR_BYTES) % SECTOR_BYTES) as usize;
    let mut filled = read_direct(reader, &mut buffer[..lead])?;
    if filled < lead {
        return Ok(filled);
    }
    let mut bounce = vec![0_u8; BOUNCE_BYTES];
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

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read, Seek, SeekFrom};

    use super::{read_file, read_full, BOUNCE_BYTES, SECTOR_BYTES};

    /// Hands out at most `step` bytes per call, like a slow device, and
    /// records where each read started.
    struct Trickle<'a> {
        data: Cursor<&'a [u8]>,
        step: usize,
        starts: Vec<u64>,
    }

    impl<'a> Trickle<'a> {
        fn new(data: &'a [u8], step: usize) -> Self {
            Self {
                data: Cursor::new(data),
                step,
                starts: Vec::new(),
            }
        }
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.starts.push(self.data.position());
            let count = self.step.min(buffer.len());
            self.data.read(&mut buffer[..count])
        }
    }

    impl Seek for Trickle<'_> {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.data.seek(position)
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn large_reads_arrive_whole_and_in_order() {
        let data = sample(BOUNCE_BYTES * 3 + 123);
        for step in [1_000, BOUNCE_BYTES, 100_000] {
            let mut buffer = vec![0_u8; data.len()];
            let read = read_full(&mut Trickle::new(&data, step), &mut buffer).unwrap();
            assert_eq!(read, data.len());
            assert_eq!(buffer, data);
            // A short file stops at its end.
            let mut longer = vec![0_u8; data.len() + 5_000];
            let read = read_full(&mut Trickle::new(&data, step), &mut longer).unwrap();
            assert_eq!(read, data.len());
            assert_eq!(&longer[..read], &data[..]);
        }
    }

    #[test]
    fn reads_from_mid_sector_reach_the_boundary_first() {
        let data = sample(BOUNCE_BYTES * 4);
        for offset in [1_u64, 37, 511, 512, 777, 4_095] {
            let mut reader = Trickle::new(&data, usize::MAX);
            reader.seek(SeekFrom::Start(offset)).unwrap();
            let mut buffer = vec![0_u8; BOUNCE_BYTES * 2 + 300];
            let read = read_full(&mut reader, &mut buffer).unwrap();
            assert_eq!(read, buffer.len());
            assert_eq!(buffer, data[offset as usize..offset as usize + read]);
            // Every read after the first starts on a whole sector.
            assert!(reader.starts[1..]
                .iter()
                .all(|start| start % SECTOR_BYTES == 0));
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
