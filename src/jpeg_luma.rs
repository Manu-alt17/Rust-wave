//! Luma-only, reduced-scale JPEG decoder, for the covers `esp_new_jpeg`
//! cannot read: in practice the progressive ones, common in downloaded
//! books (`esp_new_jpeg` decodes baseline only).
//!
//! A general progressive decoder keeps the coefficients of every component
//! at full resolution until the last scan: for a 1165x1800 cover about
//! 6.3 MB of them, more than this board can spare. This one keeps
//! - the luma component only: the panel is black and white, and the colour
//!   components' AC scans, always separate in a progressive file, are
//!   skipped without decoding;
//! - of each 8x8 block, only the coefficients the output scale needs (1 for
//!   1/8, 5 for 1/4, 25 for 1/2, all 64 at full size), plus one "non-zero"
//!   bit for each of the others, which refinement scans need to stay in step
//!   with the bitstream.
//!
//! The same cover at half size takes about 2.2 MB.
//!
//! Ported from the E-ink firmware's `image_jpeg_luma.c`, which was checked
//! against libjpeg on progressive and baseline files of every kind: at full
//! size it matches libjpeg, at reduced sizes it differs by a few grey levels
//! on sharp edges (two different reduced IDCTs), which dithering to two
//! levels erases. Here the unit tests compare it with `jpeg-decoder` on
//! files made by `tools/jpegluma/gen_fixtures.py`.
//!
//! CMYK files, and RGB ones (whose first component is not luma), are
//! refused: the caller hands them to `jpeg-decoder`.

/// Zigzag position of a coefficient -> its natural (row-major) index.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const MAX_COMPONENTS: usize = 4;

/// Coefficients, in zigzag order, needed for the top-left `n x n` corner of
/// a block: the highest zigzag position inside it, plus one.
const fn keep_for(n: usize) -> usize {
    match n {
        1 => 1,
        2 => 5,
        4 => 25,
        _ => 64,
    }
}

/// The luma plane of a JPEG, decoded at 1/8, 1/4, 1/2 or full size.
#[derive(Debug)]
pub struct LumaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub progressive: bool,
}

/// Decode the luma plane of `data` at the smallest of 1/8, 1/4, 1/2 and
/// full size that still covers `target_width x target_height` in both
/// directions (what is left of the reduction is the caller's resize), within
/// `budget_bytes` of working memory: when the scale needs more, the next
/// smaller one is used. A blurrier cover beats no cover.
pub fn decode(
    data: &[u8],
    target_width: u32,
    target_height: u32,
    budget_bytes: usize,
) -> Result<LumaImage, String> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let mut decoder = Decoder {
        bits: BitReader {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
            marker: None,
            error: false,
        },
        qt: [[0; 64]; 4],
        qt_present: [false; 4],
        dc: Default::default(),
        ac: Default::default(),
        restart_interval: 0,
        adobe_transform: None,
        frame: None,
        blocks: None,
        eobrun: 0,
        preds: [0; MAX_COMPONENTS],
    };

    let mut pos = 2;
    while pos + 1 < data.len() && !decoder.bits.error {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        // Fill bytes before a marker.
        while pos + 1 < data.len() && data[pos + 1] == 0xFF {
            pos += 1;
        }
        if pos + 1 >= data.len() {
            break;
        }
        let marker = data[pos + 1];
        pos += 2;
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xD9 {
            break;
        }
        if pos + 2 > data.len() {
            break;
        }
        let length = usize::from(u16::from_be_bytes([data[pos], data[pos + 1]]));
        if length < 2 || pos + length > data.len() {
            return Err("truncated segment".into());
        }
        let segment = &data[pos + 2..pos + length];
        match marker {
            0xDB => decoder.parse_dqt(segment)?,
            0xC4 => decoder.parse_dht(segment)?,
            0xDD if segment.len() >= 2 => {
                decoder.restart_interval =
                    usize::from(u16::from_be_bytes([segment[0], segment[1]]));
            }
            0xEE if segment.len() >= 12 && segment.starts_with(b"Adobe") => {
                decoder.adobe_transform = Some(segment[11]);
            }
            0xC0..=0xC2 => {
                if decoder.frame.is_some() {
                    return Err("second frame header".into());
                }
                decoder.parse_sof(
                    segment,
                    marker == 0xC2,
                    target_width,
                    target_height,
                    budget_bytes,
                )?;
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(format!("unsupported JPEG process (SOF 0x{marker:02X})"));
            }
            0xDA => {
                pos = decoder.parse_sos_and_decode(segment, pos + length)?;
                continue;
            }
            _ => {}
        }
        pos += length;
    }

    if decoder.bits.error {
        return Err("corrupt entropy-coded data".into());
    }
    let (Some(frame), Some(blocks)) = (decoder.frame.as_ref(), decoder.blocks.as_ref()) else {
        return Err("no frame header".into());
    };
    let table = frame.components[0].tq;
    if !decoder.qt_present[table] {
        return Err("luma quantization table missing".into());
    }
    reconstruct(frame, blocks, &decoder.qt[table])
}

#[derive(Clone, Copy, Debug, Default)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
}

#[derive(Clone)]
struct Huffman {
    present: bool,
    maxcode: [i32; 18],
    mincode: [i32; 17],
    valptr: [u16; 17],
    symbols: [u8; 256],
}

impl Default for Huffman {
    fn default() -> Self {
        Self {
            present: false,
            maxcode: [-1; 18],
            mincode: [0; 17],
            valptr: [0; 17],
            symbols: [0; 256],
        }
    }
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    components: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    /// Luma plane size in pixels, and in blocks padded to whole MCUs.
    y_w: usize,
    y_h: usize,
    bw: usize,
    bh: usize,
}

/// What is kept of each luma block.
struct Blocks {
    /// Side of the reconstructed block: 1, 2, 4 or 8.
    n: usize,
    /// Coefficients kept per block, in zigzag order (see [`keep_for`]).
    keep: usize,
    coef: Vec<i16>,
    /// Bit `k` set: zigzag coefficient `k` is non-zero, kept or not.
    nz: Vec<u64>,
}

impl Blocks {
    fn set(&mut self, index: usize, k: usize, value: i32) {
        if k < self.keep {
            self.coef[index * self.keep + k] = value as i16;
        }
        if value != 0 {
            self.nz[index] |= 1 << k;
        }
    }

    /// One correction bit of an AC refinement scan for a coefficient that is
    /// already non-zero: read even when the coefficient is not kept.
    fn refine(&mut self, bits: &mut BitReader<'_>, index: usize, k: usize, p1: i32, m1: i32) {
        if bits.bit() != 0 && k < self.keep {
            let slot = &mut self.coef[index * self.keep + k];
            let value = i32::from(*slot);
            if value & p1 == 0 {
                *slot = (value + if value >= 0 { p1 } else { m1 }) as i16;
            }
        }
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    nbits: u32,
    /// The marker that ended this scan's entropy-coded data, once reached.
    marker: Option<u8>,
    error: bool,
}

impl BitReader<'_> {
    fn start(&mut self, pos: usize) {
        self.pos = pos;
        self.acc = 0;
        self.nbits = 0;
        self.marker = None;
    }

    fn fill(&mut self) {
        while self.nbits <= 24 {
            let mut byte = 0;
            if self.marker.is_none() && self.pos < self.data.len() {
                byte = u32::from(self.data[self.pos]);
                if byte == 0xFF {
                    let next = self.data.get(self.pos + 1).copied().unwrap_or(0);
                    if next == 0x00 {
                        // A data 0xFF, stuffed with a zero.
                        self.pos += 2;
                    } else {
                        self.marker = Some(next);
                        byte = 0;
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acc = (self.acc << 8) | byte;
            self.nbits += 8;
        }
    }

    fn bit(&mut self) -> u32 {
        if self.nbits == 0 {
            self.fill();
        }
        self.nbits -= 1;
        (self.acc >> self.nbits) & 1
    }

    fn bits(&mut self, count: u32) -> i32 {
        if count == 0 {
            return 0;
        }
        if count > 16 {
            // Only in a corrupt file: more bits than an 8-bit sample's
            // coefficient can have.
            self.error = true;
            return 0;
        }
        if self.nbits < count {
            self.fill();
        }
        self.nbits -= count;
        ((self.acc >> self.nbits) & ((1 << count) - 1)) as i32
    }

    fn huffman(&mut self, table: &Huffman) -> u8 {
        if !table.present {
            // A table the scan names but the file never defines.
            self.error = true;
            return 0;
        }
        let mut code = 0_i32;
        for length in 1..=16 {
            code = (code << 1) | self.bit() as i32;
            if code <= table.maxcode[length] {
                let index = i32::from(table.valptr[length]) + code - table.mincode[length];
                if !(0..=255).contains(&index) {
                    break;
                }
                return table.symbols[index as usize];
            }
        }
        self.error = true;
        0
    }

    /// Byte-align and step over the expected RSTn marker.
    fn restart(&mut self) {
        self.acc = 0;
        self.nbits = 0;
        if matches!(self.marker, Some(0xD0..=0xD7)) {
            self.pos += 2;
            self.marker = None;
            return;
        }
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                break;
            }
            self.pos += 1;
        }
        self.marker = None;
    }

    /// Position of the first real marker (not stuffing, not RSTn) at or
    /// after `from`.
    fn end_pos(&self, from: usize) -> usize {
        let data = self.data;
        (from..data.len().saturating_sub(1))
            .find(|&p| {
                let marker = data[p + 1];
                data[p] == 0xFF && marker != 0x00 && !(0xD0..=0xD7).contains(&marker)
            })
            .unwrap_or(data.len())
    }
}

#[inline]
fn extend(value: i32, size: u32) -> i32 {
    // Sizes over 16 only come from corrupt data, already flagged by `bits`.
    if (1..=16).contains(&size) && value < (1 << (size - 1)) {
        value - (1 << size) + 1
    } else {
        value
    }
}

/// Spectral selection and successive approximation of one scan.
#[derive(Clone, Copy)]
struct Scan {
    ss: usize,
    se: usize,
    ah: u32,
    al: u32,
}

/// One block of one component. `index` is `None` for a colour component's
/// block, read only to stay in step with the bitstream.
#[allow(clippy::too_many_arguments)]
fn decode_block(
    bits: &mut BitReader<'_>,
    dc_table: &Huffman,
    ac_table: &Huffman,
    progressive: bool,
    pred: &mut i32,
    eobrun: &mut u32,
    blocks: &mut Blocks,
    index: Option<usize>,
    scan: Scan,
) {
    if !progressive {
        let size = u32::from(bits.huffman(dc_table));
        *pred = pred.wrapping_add(extend(bits.bits(size), size));
        if let Some(index) = index {
            blocks.set(index, 0, *pred);
        }
        let mut k = 1;
        while k < 64 && !bits.error {
            let rs = bits.huffman(ac_table);
            let run = usize::from(rs >> 4);
            let size = u32::from(rs & 15);
            if size == 0 {
                if run != 15 {
                    break;
                }
                k += 16;
                continue;
            }
            k += run;
            let value = extend(bits.bits(size), size);
            if k > 63 {
                bits.error = true;
                return;
            }
            if let Some(index) = index {
                blocks.set(index, k, value);
            }
            k += 1;
        }
        return;
    }

    if scan.ss == 0 {
        // DC scans, interleaved or not.
        if scan.ah == 0 {
            let size = u32::from(bits.huffman(dc_table));
            *pred = pred.wrapping_add(extend(bits.bits(size), size));
            if let Some(index) = index {
                blocks.set(index, 0, pred.wrapping_shl(scan.al));
            }
        } else if bits.bit() != 0 {
            if let Some(index) = index {
                blocks.coef[index * blocks.keep] |= 1 << scan.al;
                blocks.nz[index] |= 1;
            }
        }
        return;
    }

    // AC scans: always of a single component, here always luma.
    let Some(index) = index else {
        return;
    };
    if scan.ah == 0 {
        if *eobrun > 0 {
            *eobrun -= 1;
            return;
        }
        let mut k = scan.ss;
        while k <= scan.se && !bits.error {
            let rs = bits.huffman(ac_table);
            let run = u32::from(rs >> 4);
            let size = u32::from(rs & 15);
            if size == 0 {
                if run < 15 {
                    *eobrun = (1 << run) - 1;
                    if run > 0 {
                        *eobrun += bits.bits(run) as u32;
                    }
                    break;
                }
                k += 16;
                continue;
            }
            k += run as usize;
            if k > 63 {
                bits.error = true;
                return;
            }
            let value = extend(bits.bits(size), size) * (1 << scan.al);
            blocks.set(index, k, value);
            k += 1;
        }
        return;
    }

    // AC refinement, the delicate part: every coefficient of the band that
    // is already non-zero gets a correction bit, kept or not -- which is
    // why every coefficient has its non-zero bit.
    let p1 = 1 << scan.al;
    let m1 = -(1 << scan.al);
    let mut k = scan.ss;
    if *eobrun == 0 {
        while k <= scan.se && !bits.error {
            let rs = bits.huffman(ac_table);
            let mut run = u32::from(rs >> 4);
            let size = rs & 15;
            let mut value = 0;
            if size != 0 {
                value = if bits.bit() != 0 { p1 } else { m1 };
            } else if run != 15 {
                *eobrun = 1 << run;
                if run > 0 {
                    *eobrun += bits.bits(run) as u32;
                }
                break;
            }
            while k <= scan.se {
                if (blocks.nz[index] >> k) & 1 != 0 {
                    blocks.refine(bits, index, k, p1, m1);
                } else {
                    if run == 0 {
                        break;
                    }
                    run -= 1;
                }
                k += 1;
            }
            if value != 0 && k <= scan.se {
                blocks.set(index, k, value);
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= scan.se {
            if (blocks.nz[index] >> k) & 1 != 0 {
                blocks.refine(bits, index, k, p1, m1);
            }
            k += 1;
        }
        *eobrun -= 1;
    }
}

struct Decoder<'a> {
    bits: BitReader<'a>,
    qt: [[u16; 64]; 4],
    qt_present: [bool; 4],
    dc: [Huffman; 4],
    ac: [Huffman; 4],
    restart_interval: usize,
    /// APP14 "Adobe" colour transform: 0 means the components are RGB.
    adobe_transform: Option<u8>,
    frame: Option<Frame>,
    blocks: Option<Blocks>,
    eobrun: u32,
    preds: [i32; MAX_COMPONENTS],
}

impl Decoder<'_> {
    fn parse_dqt(&mut self, segment: &[u8]) -> Result<(), String> {
        let mut p = 0;
        while p < segment.len() {
            let precision = segment[p] >> 4;
            let table = usize::from(segment[p] & 15);
            p += 1;
            let size = if precision != 0 { 128 } else { 64 };
            if table > 3 || p + size > segment.len() {
                return Err("bad quantization table".into());
            }
            for i in 0..64 {
                self.qt[table][i] = if precision != 0 {
                    u16::from_be_bytes([segment[p + 2 * i], segment[p + 2 * i + 1]])
                } else {
                    u16::from(segment[p + i])
                };
            }
            self.qt_present[table] = true;
            p += size;
        }
        Ok(())
    }

    fn parse_dht(&mut self, segment: &[u8]) -> Result<(), String> {
        let mut p = 0;
        while p + 17 <= segment.len() {
            let class = segment[p] >> 4;
            let id = usize::from(segment[p] & 15);
            if id > 3 || class > 1 {
                return Err("bad Huffman table".into());
            }
            let counts = &segment[p + 1..p + 17];
            let total: usize = counts.iter().map(|&count| usize::from(count)).sum();
            if total > 256 || p + 17 + total > segment.len() {
                return Err("bad Huffman table".into());
            }
            let table = if class == 1 {
                &mut self.ac[id]
            } else {
                &mut self.dc[id]
            };
            table.symbols[..total].copy_from_slice(&segment[p + 17..p + 17 + total]);
            let mut code = 0_i32;
            let mut k = 0_u16;
            for length in 1..=16 {
                let count = counts[length - 1];
                table.maxcode[length] = -1;
                if count > 0 {
                    table.valptr[length] = k;
                    table.mincode[length] = code;
                    code += i32::from(count);
                    k += u16::from(count);
                    table.maxcode[length] = code - 1;
                }
                code <<= 1;
            }
            table.maxcode[17] = i32::MAX;
            table.present = true;
            p += 17 + total;
        }
        Ok(())
    }

    fn parse_sof(
        &mut self,
        segment: &[u8],
        progressive: bool,
        target_width: u32,
        target_height: u32,
        budget_bytes: usize,
    ) -> Result<(), String> {
        if segment.len() < 6 || segment[0] != 8 {
            return Err("only 8-bit JPEGs are supported".into());
        }
        let height = usize::from(u16::from_be_bytes([segment[1], segment[2]]));
        let width = usize::from(u16::from_be_bytes([segment[3], segment[4]]));
        let count = usize::from(segment[5]);
        if width == 0 || height == 0 || segment.len() < 6 + 3 * count {
            return Err("bad frame header".into());
        }
        // Only YCbCr and grayscale carry luma in their first component.
        match count {
            1 | 3 => {}
            4 => return Err("CMYK JPEG".into()),
            _ => return Err(format!("JPEG with {count} components")),
        }
        let mut components = Vec::with_capacity(count);
        for i in 0..count {
            let factors = segment[7 + 3 * i];
            let component = Component {
                id: segment[6 + 3 * i],
                h: usize::from(factors >> 4),
                v: usize::from(factors & 15),
                tq: usize::from(segment[8 + 3 * i] & 3),
            };
            if !(1..=4).contains(&component.h) || !(1..=4).contains(&component.v) {
                return Err("bad sampling factors".into());
            }
            components.push(component);
        }
        let rgb_ids = components.iter().map(|c| c.id).eq(*b"RGB");
        if count == 3 && (self.adobe_transform == Some(0) || rgb_ids) {
            return Err("RGB JPEG".into());
        }
        let hmax = components.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = components.iter().map(|c| c.v).max().unwrap_or(1);
        let mcux = width.div_ceil(8 * hmax);
        let mcuy = height.div_ceil(8 * vmax);
        let luma = components[0];
        let frame = Frame {
            width,
            height,
            progressive,
            components,
            hmax,
            vmax,
            mcux,
            mcuy,
            y_w: (width * luma.h).div_ceil(hmax),
            y_h: (height * luma.v).div_ceil(vmax),
            bw: mcux * luma.h,
            bh: mcuy * luma.v,
        };
        self.blocks = Some(allocate(&frame, target_width, target_height, budget_bytes)?);
        self.frame = Some(frame);
        Ok(())
    }

    /// Read a scan header, decode the scan, and return where the next
    /// marker starts.
    fn parse_sos_and_decode(&mut self, segment: &[u8], data_start: usize) -> Result<usize, String> {
        let Some(frame) = self.frame.as_ref() else {
            return Err("scan before frame header".into());
        };
        let count = usize::from(*segment.first().ok_or("bad scan header")?);
        if !(1..=MAX_COMPONENTS).contains(&count) || segment.len() < 1 + 2 * count + 3 {
            return Err("bad scan header".into());
        }
        let mut members = Vec::with_capacity(count);
        for i in 0..count {
            let id = segment[1 + 2 * i];
            let component = frame
                .components
                .iter()
                .position(|c| c.id == id)
                .ok_or("scan names an unknown component")?;
            let tables = segment[2 + 2 * i];
            members.push((
                component,
                usize::from(tables >> 4) & 3,
                usize::from(tables & 15) & 3,
            ));
        }
        let scan = Scan {
            ss: usize::from(segment[1 + 2 * count]),
            se: usize::from(segment[2 + 2 * count]),
            ah: u32::from(segment[3 + 2 * count] >> 4),
            al: u32::from(segment[3 + 2 * count] & 15),
        };
        if scan.se > 63 || scan.ss > scan.se || scan.al > 13 {
            return Err("bad spectral selection".into());
        }
        Ok(self.decode_scan(data_start, &members, scan))
    }

    fn decode_scan(
        &mut self,
        start: usize,
        members: &[(usize, usize, usize)],
        scan: Scan,
    ) -> usize {
        let Decoder {
            bits,
            dc,
            ac,
            restart_interval,
            frame,
            blocks,
            eobrun,
            preds,
            ..
        } = self;
        let (Some(frame), Some(blocks)) = (frame.as_ref(), blocks.as_mut()) else {
            return start;
        };
        bits.start(start);
        *eobrun = 0;
        *preds = [0; MAX_COMPONENTS];
        if !members.iter().any(|&(component, _, _)| component == 0) {
            // Colour components only: skip to the next marker without
            // decoding anything.
            return bits.end_pos(start);
        }

        let interval = *restart_interval;
        let mut count = 0_usize;
        let restart_if_due = |bits: &mut BitReader<'_>,
                              count: usize,
                              preds: &mut [i32; MAX_COMPONENTS],
                              eobrun: &mut u32| {
            if interval > 0 && count > 0 && count % interval == 0 {
                bits.restart();
                *preds = [0; MAX_COMPONENTS];
                *eobrun = 0;
            }
        };

        if let [(component, td, ta)] = *members {
            // Not interleaved: one block per MCU, over the component's own
            // size in blocks.
            let c = frame.components[component];
            let blocks_w = (frame.width * c.h).div_ceil(frame.hmax).div_ceil(8);
            let blocks_h = (frame.height * c.v).div_ceil(frame.vmax).div_ceil(8);
            'rows: for by in 0..blocks_h {
                for bx in 0..blocks_w {
                    if bits.error {
                        break 'rows;
                    }
                    restart_if_due(bits, count, preds, eobrun);
                    let index = (component == 0).then_some(by * frame.bw + bx);
                    decode_block(
                        bits,
                        &dc[td],
                        &ac[ta],
                        frame.progressive,
                        &mut preds[component],
                        eobrun,
                        blocks,
                        index,
                        scan,
                    );
                    count += 1;
                }
            }
        } else {
            'mcus: for my in 0..frame.mcuy {
                for mx in 0..frame.mcux {
                    if bits.error {
                        break 'mcus;
                    }
                    restart_if_due(bits, count, preds, eobrun);
                    for &(component, td, ta) in members {
                        let c = frame.components[component];
                        for v in 0..c.v {
                            for h in 0..c.h {
                                let index = (component == 0)
                                    .then_some((my * c.v + v) * frame.bw + mx * c.h + h);
                                decode_block(
                                    bits,
                                    &dc[td],
                                    &ac[ta],
                                    frame.progressive,
                                    &mut preds[component],
                                    eobrun,
                                    blocks,
                                    index,
                                    scan,
                                );
                            }
                        }
                    }
                    count += 1;
                }
            }
        }
        bits.end_pos(bits.pos)
    }
}

/// Side of the reconstructed block for the smallest scale that still covers
/// the target in both directions.
fn pick_scale(frame: &Frame, target_width: u32, target_height: u32) -> usize {
    let (target_width, target_height) = (target_width as usize, target_height as usize);
    if target_width == 0 || target_height == 0 {
        return 8;
    }
    [1, 2, 4]
        .into_iter()
        .find(|&n| {
            (frame.y_w * n).div_ceil(8) >= target_width
                && (frame.y_h * n).div_ceil(8) >= target_height
        })
        .unwrap_or(8)
}

/// Working memory for `frame`: the chosen scale, or the largest smaller one
/// that fits in `budget_bytes` and can actually be allocated. Allocation
/// failures are reported, never fatal.
fn allocate(
    frame: &Frame,
    target_width: u32,
    target_height: u32,
    budget_bytes: usize,
) -> Result<Blocks, String> {
    // In u64: on the ESP32-S3 `usize` is 32 bits, and a 65535x65535 frame
    // header would overflow it.
    let blocks = frame.bw as u64 * frame.bh as u64;
    let mut n = pick_scale(frame, target_width, target_height);
    loop {
        let keep = keep_for(n);
        let pixels = ((frame.y_w * n).div_ceil(8) as u64) * ((frame.y_h * n).div_ceil(8) as u64);
        let needed = blocks * (keep as u64 * 2 + 8) + pixels;
        if needed <= budget_bytes as u64 {
            // Both fit in `usize`: they are smaller than the budget.
            let (block_count, coef_count) = (blocks as usize, blocks as usize * keep);
            let mut coef = Vec::new();
            let mut nz = Vec::new();
            if coef.try_reserve_exact(coef_count).is_ok()
                && nz.try_reserve_exact(block_count).is_ok()
            {
                coef.resize(coef_count, 0);
                nz.resize(block_count, 0);
                return Ok(Blocks { n, keep, coef, nz });
            }
        }
        if n == 1 {
            return Err(format!(
                "{}x{} JPEG needs {needed} bytes even at 1/8, over the {budget_bytes} byte budget",
                frame.width, frame.height
            ));
        }
        n /= 2;
    }
}

/// Reduced IDCT: the ordinary 8x8 IDCT sampled at the centre of each group
/// of 8/n pixels, that is with the cosines cos((2x+1)u*pi/(2n)), using only
/// the coefficients u, v < n. Separable: rows first, then columns.
fn reconstruct(frame: &Frame, blocks: &Blocks, quant: &[u16; 64]) -> Result<LumaImage, String> {
    let n = blocks.n;
    let out_w = (frame.y_w * n).div_ceil(8);
    let out_h = (frame.y_h * n).div_ceil(8);
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(out_w * out_h)
        .map_err(|_| format!("no memory for a {out_w}x{out_h} luma plane"))?;
    pixels.resize(out_w * out_h, 0);

    let mut cosines = [[0_f32; 8]; 8];
    for (x, row) in cosines.iter_mut().enumerate().take(n) {
        for (u, cosine) in row.iter_mut().enumerate().take(n) {
            let scale = if u == 0 {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            *cosine =
                scale * (((2 * x + 1) * u) as f32 * std::f32::consts::PI / (2 * n) as f32).cos();
        }
    }

    for by in 0..frame.bh {
        for bx in 0..frame.bw {
            let base = (by * frame.bw + bx) * blocks.keep;
            let mut coefficients = [[0_f32; 8]; 8];
            for (k, &value) in blocks.coef[base..base + blocks.keep].iter().enumerate() {
                if value != 0 {
                    let natural = usize::from(ZIGZAG[k]);
                    let (v, u) = (natural / 8, natural % 8);
                    if u < n && v < n {
                        coefficients[v][u] = f32::from(value) * f32::from(quant[k]);
                    }
                }
            }
            let mut rows = [[0_f32; 8]; 8];
            for v in 0..n {
                for x in 0..n {
                    rows[v][x] = (0..n).map(|u| cosines[x][u] * coefficients[v][u]).sum();
                }
            }
            for y in 0..n {
                let py = by * n + y;
                if py >= out_h {
                    break;
                }
                for x in 0..n {
                    let px = bx * n + x;
                    if px >= out_w {
                        break;
                    }
                    let sample: f32 = (0..n).map(|v| cosines[y][v] * rows[v][x]).sum();
                    pixels[py * out_w + px] =
                        (sample / 4.0 + 128.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    Ok(LumaImage {
        width: out_w as u32,
        height: out_h as u32,
        pixels,
        progressive: frame.progressive,
    })
}

#[cfg(test)]
mod tests {
    use super::{decode, LumaImage};

    const UNLIMITED: usize = usize::MAX;

    macro_rules! fixture {
        ($name:literal) => {
            (
                $name,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/testdata/jpeg/",
                    $name
                ))
                .as_slice(),
            )
        };
    }

    fn fixtures() -> [(&'static str, &'static [u8]); 7] {
        [
            fixture!("baseline_420.jpg"),
            fixture!("baseline_422_restart.jpg"),
            fixture!("progressive_420.jpg"),
            fixture!("progressive_422.jpg"),
            fixture!("progressive_444_q95.jpg"),
            fixture!("progressive_gray.jpg"),
            fixture!("progressive_optimized_restart.jpg"),
        ]
    }

    /// jpeg-decoder's luma plane at IDCT size `n` (of 8). Its "no colour
    /// transform" mode, which would hand back Y as decoded, overruns its
    /// output buffer in 0.3.2, so Y is recovered from its RGB output with
    /// the inverse of the BT.601 matrix it uses: exact to rounding, except
    /// where a channel was clamped at 0 or 255 (`None`, not compared).
    fn reference(bytes: &[u8], n: u16) -> (u32, u32, Vec<Option<u8>>) {
        let mut decoder = jpeg_decoder::Decoder::new(bytes);
        decoder.read_info().unwrap();
        let info = decoder.info().unwrap();
        decoder
            .scale((info.width * n).div_ceil(8), (info.height * n).div_ceil(8))
            .unwrap();
        let pixels = decoder.decode().unwrap();
        let info = decoder.info().unwrap();
        let luma = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => pixels.into_iter().map(Some).collect(),
            jpeg_decoder::PixelFormat::RGB24 => pixels
                .chunks_exact(3)
                .map(|rgb| {
                    let clamped = rgb.iter().any(|&channel| channel == 0 || channel == 255);
                    let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(f64::from);
                    (!clamped).then(|| (0.299 * r + 0.587 * g + 0.114 * b).round() as u8)
                })
                .collect(),
            other => panic!("unexpected pixel format {other:?}"),
        };
        (u32::from(info.width), u32::from(info.height), luma)
    }

    /// Mean and largest difference over the comparable pixels.
    fn differences(ours: &LumaImage, reference: &[Option<u8>]) -> (f64, u8) {
        let pairs: Vec<(u8, u8)> = ours
            .pixels
            .iter()
            .zip(reference)
            .filter_map(|(&a, &b)| b.map(|b| (a, b)))
            .collect();
        assert!(
            pairs.len() * 10 >= ours.pixels.len() * 7,
            "too few comparable pixels"
        );
        let total: u64 = pairs.iter().map(|&(a, b)| u64::from(a.abs_diff(b))).sum();
        let max = pairs.iter().map(|&(a, b)| a.abs_diff(b)).max().unwrap_or(0);
        (total as f64 / pairs.len() as f64, max)
    }

    #[test]
    fn full_size_matches_jpeg_decoder() {
        for (name, bytes) in fixtures() {
            let ours = decode(bytes, 173, 229, UNLIMITED).unwrap();
            let (width, height, expected) = reference(bytes, 8);
            assert_eq!((ours.width, ours.height), (width, height), "{name}");
            let (mean, max) = differences(&ours, &expected);
            assert!(mean < 0.1 && max <= 2, "{name}: mean {mean:.3} max {max}");
        }
    }

    #[test]
    fn reduced_sizes_stay_close_to_jpeg_decoder() {
        for (name, bytes) in fixtures() {
            for n in [4_u16, 2, 1] {
                let (width, height, expected) = reference(bytes, n);
                let ours = decode(bytes, width, height, UNLIMITED).unwrap();
                assert_eq!(
                    (ours.width, ours.height),
                    (width, height),
                    "{name} 1/{}",
                    8 / n
                );
                let (mean, max) = differences(&ours, &expected);
                assert!(
                    mean < 1.0 && max <= 3,
                    "{name} 1/{}: mean {mean:.3} max {max}",
                    8 / n
                );
            }
        }
    }

    #[test]
    fn picks_the_smallest_scale_covering_the_target() {
        let (_, bytes) = fixture!("progressive_420.jpg");
        // 173x229: 1/8 is 22x29, 1/4 is 44x58, 1/2 is 87x115.
        assert_eq!(decode(bytes, 22, 29, UNLIMITED).unwrap().width, 22);
        assert_eq!(decode(bytes, 23, 29, UNLIMITED).unwrap().width, 44);
        assert_eq!(decode(bytes, 44, 100, UNLIMITED).unwrap().width, 87);
        assert_eq!(decode(bytes, 300, 300, UNLIMITED).unwrap().width, 173);
    }

    #[test]
    fn a_tight_budget_steps_down_the_scale_instead_of_failing() {
        let (_, bytes) = fixture!("progressive_420.jpg");
        // 22x30 blocks: full size needs 660 * 136 + 173 * 229 bytes.
        let full = decode(bytes, 173, 229, 200_000).unwrap();
        assert_eq!(full.width, 173);
        let half = decode(bytes, 173, 229, 50_000).unwrap();
        assert_eq!((half.width, half.height), (87, 115));
        assert!(decode(bytes, 173, 229, 1_000).is_err());
    }

    #[test]
    fn cmyk_and_rgb_files_are_left_to_jpeg_decoder() {
        let (_, cmyk) = fixture!("cmyk.jpg");
        assert_eq!(
            decode(cmyk, 100, 100, UNLIMITED).err().unwrap(),
            "CMYK JPEG"
        );
        // The same YCbCr file with its components renamed R, G and B: the
        // first one would then be red, not luma.
        let (_, bytes) = fixture!("progressive_420.jpg");
        let sof = bytes
            .windows(2)
            .position(|pair| pair == [0xFF, 0xC2])
            .unwrap();
        let mut rgb = bytes.to_vec();
        for (i, id) in b"RGB".iter().enumerate() {
            rgb[sof + 10 + 3 * i] = *id;
        }
        assert_eq!(decode(&rgb, 100, 100, UNLIMITED).err().unwrap(), "RGB JPEG");
        assert!(decode(b"not a jpeg", 100, 100, UNLIMITED).is_err());
    }

    #[test]
    fn truncated_and_corrupt_files_never_panic() {
        for (name, bytes) in fixtures() {
            for cut in (0..bytes.len()).step_by(97) {
                let _ = decode(&bytes[..cut], 100, 100, UNLIMITED);
            }
            for seed in 0..8_usize {
                let mut corrupt = bytes.to_vec();
                for i in (bytes.len() / 4 + seed..bytes.len()).step_by(89 + seed) {
                    corrupt[i] ^= 0x5A;
                }
                if let Ok(image) = decode(&corrupt, 100, 100, UNLIMITED) {
                    assert_eq!(
                        image.pixels.len(),
                        (image.width * image.height) as usize,
                        "{name}"
                    );
                }
            }
        }
        // A frame header announcing 65535x65535 is refused, not allocated.
        let (_, bytes) = fixture!("progressive_420.jpg");
        let sof = bytes
            .windows(2)
            .position(|pair| pair == [0xFF, 0xC2])
            .unwrap();
        let mut huge = bytes.to_vec();
        huge[sof + 5..sof + 9].copy_from_slice(&[0xFF; 4]);
        assert!(decode(&huge, 480, 800, 4 * 1024 * 1024).is_err());
    }
}
