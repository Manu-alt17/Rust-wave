//! Thin safe wrapper over the Helix fixed-point MP3 decoder, packaged for
//! ESP-IDF as `chmorgan/esp-libhelix-mp3` (see Cargo.toml's
//! `extra_components`): the decoder the Waveshare audio examples for this
//! board use.

use esp_idf_svc::sys::libhelix_mp3 as ffi;

/// Interleaved samples one frame can decode to: 1152 per channel, stereo.
pub const MAX_FRAME_SAMPLES: usize = 1152 * 2;
/// Input bytes one frame can span (Helix's `MAINBUF_SIZE`).
pub const MAX_FRAME_BYTES: usize = 1940;

/// Helix's `ERR_MP3_INDATA_UNDERFLOW`: the frame runs past the input.
const ERR_INDATA_UNDERFLOW: i32 = -1;
/// Helix's `ERR_MP3_MAINDATA_UNDERFLOW`: the frame needs bit-reservoir
/// data from frames before it, as the first frames after a seek do.
const ERR_MAINDATA_UNDERFLOW: i32 = -2;

/// One decoded frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedFrame {
    /// Interleaved samples written to the output buffer.
    pub samples: usize,
    pub channels: u8,
    pub sample_rate: u32,
}

/// Result of one [`Mp3Decoder::decode`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeStep {
    Frame(DecodedFrame),
    /// The input holds no complete frame: read more first.
    NeedMoreInput,
    /// A frame was consumed without output (bit reservoir still filling
    /// after a seek, or a damaged frame): just decode the next one.
    Skipped,
}

pub struct Mp3Decoder {
    handle: ffi::HMP3Decoder,
}

// The decoder is plain heap state touched only through `&mut self`.
unsafe impl Send for Mp3Decoder {}

impl Mp3Decoder {
    /// `None` if Helix could not allocate its ~23 KB of state.
    #[must_use]
    pub fn new() -> Option<Self> {
        let handle = unsafe { ffi::MP3InitDecoder() };
        (!handle.is_null()).then_some(Self { handle })
    }

    /// Decode the next frame from `input`, advancing it past whatever was
    /// consumed. `output` receives interleaved 16-bit samples.
    pub fn decode(
        &mut self,
        input: &mut &[u8],
        output: &mut [i16; MAX_FRAME_SAMPLES],
    ) -> DecodeStep {
        let sync = unsafe { ffi::MP3FindSyncWord(input.as_ptr().cast_mut(), input.len() as i32) };
        if sync < 0 {
            // No frame start anywhere: keep the last bytes, which may hold
            // the first half of one.
            let keep = input.len().min(3);
            *input = &input[input.len() - keep..];
            return DecodeStep::NeedMoreInput;
        }
        *input = &input[sync as usize..];
        let mut cursor = input.as_ptr().cast_mut();
        let mut left = input.len() as i32;
        let result =
            unsafe { ffi::MP3Decode(self.handle, &mut cursor, &mut left, output.as_mut_ptr(), 0) };
        match result {
            0 => {
                *input = &input[input.len() - left as usize..];
                // Plain integers: all-zero is a valid value to fill in.
                let mut info: ffi::MP3FrameInfo = unsafe { core::mem::zeroed() };
                unsafe { ffi::MP3GetLastFrameInfo(self.handle, &mut info) };
                DecodeStep::Frame(DecodedFrame {
                    samples: info.outputSamps.clamp(0, MAX_FRAME_SAMPLES as i32) as usize,
                    channels: info.nChans.clamp(1, 2) as u8,
                    sample_rate: info.samprate.max(0) as u32,
                })
            }
            ERR_INDATA_UNDERFLOW => DecodeStep::NeedMoreInput,
            ERR_MAINDATA_UNDERFLOW => {
                *input = &input[input.len() - left as usize..];
                DecodeStep::Skipped
            }
            _ => {
                // A damaged frame: step past its sync word and resync.
                *input = &input[1.min(input.len())..];
                DecodeStep::Skipped
            }
        }
    }
}

impl Drop for Mp3Decoder {
    fn drop(&mut self) {
        unsafe { ffi::MP3FreeDecoder(self.handle) };
    }
}
