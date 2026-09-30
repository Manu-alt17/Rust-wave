//! The audio engine: one thread that owns the ES8311 codec, the speaker
//! amplifier and the I2S0 TX channel, and streams either the test tone or
//! an audiobook.
//!
//! The main loop sends it [`AudioCommand`]s and reads its state back with
//! [`AudioEngine::poll`]. Decoding and the blocking I2S writes that pace
//! playback happen here, pinned to the second core, so the UI keeps
//! responding -- and the e-paper keeps refreshing -- while a book plays.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::{mpsc, Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use embedded_hal::i2c::I2c;
use esp_idf_svc::hal::{
    delay::{FreeRtos, TickType},
    gpio::AnyIOPin,
    i2s::{
        config::{
            ClockSource, Config as I2sChannelConfig, DataBitWidth, MclkMultiple, SlotMode,
            StdClkConfig, StdConfig, StdGpioConfig, StdSlotConfig,
        },
        I2sDriver, I2sTx, I2S0,
    },
};
use log::{info, warn};

use crate::audiobook::{
    probe_mp3_at, Audiobook, ListeningPosition, Mp3Probe, Mp3StreamInfo, NowPlaying, PlayerState,
    MP3_PROBE_BYTES,
};

use super::{
    espidf::AudioRuntime,
    mp3::{DecodeStep, Mp3Decoder, MAX_FRAME_BYTES, MAX_FRAME_SAMPLES},
    tone::{ChimeGenerator, PCM_CHUNK_BYTES},
    AudioPlaybackState, AudioSnapshot, AudioUiRequest, AUDIO_BCLK_GPIO, AUDIO_DOUT_GPIO,
    AUDIO_MCLK_GPIO, AUDIO_SAMPLE_RATE_HZ, AUDIO_VOLUME_STEP_PERCENT, AUDIO_WS_GPIO,
};

/// Above the main task, so a busy UI never starves the DMA buffers.
const AUDIO_THREAD_PRIORITY: i32 = 6;
/// The main loop runs on core 0 with Wi-Fi; decoding gets core 1.
const AUDIO_THREAD_CORE: i32 = 1;
const AUDIO_THREAD_STACK_BYTES: usize = 12 * 1024;
/// How long an idle engine waits for a command before looking again.
const IDLE_WAIT: Duration = Duration::from_millis(500);
/// Position-only status updates, while playing, at most this often.
const POSITION_PUBLISH_INTERVAL: Duration = Duration::from_millis(1000);
/// Longest a single I2S write may block. Playback writes return as soon as
/// the DMA ring has room, every ~20 ms; this only bounds a stalled channel.
const I2S_WRITE_TIMEOUT_MS: u64 = 1_000;
/// DMA ring: 6 buffers of 480 stereo frames, ~65 ms at 44.1 kHz, enough to
/// ride out an SD card read without a gap.
const DMA_BUFFERS: u32 = 6;
const DMA_FRAMES_PER_BUFFER: u32 = 480;
/// MP3 bytes held in RAM ahead of the decoder.
const INPUT_BUFFER_BYTES: usize = 16 * 1024;
/// "Previous track" restarts the current one once this far into it.
const RESTART_TRACK_AFTER_MS: u64 = 5_000;

/// What the main loop asks of the engine.
pub enum AudioCommand {
    /// Settings > Audio: test tone, stop, volume, mute.
    Ui(AudioUiRequest),
    Player(PlayerCommand),
    /// Stop all output before the codec's power rail is cut, then reply.
    Suspend(mpsc::SyncSender<()>),
    /// Reprogram the codec after its rail came back, then reply.
    Restore(mpsc::SyncSender<Result<(), String>>),
}

pub enum PlayerCommand {
    Play {
        book: Audiobook,
        position: ListeningPosition,
    },
    TogglePause,
    SeekBy {
        seconds: i32,
    },
    SkipTrack {
        forward: bool,
    },
    Stop,
}

/// Everything the UI shows about audio, with a counter that changes
/// whenever anything in it does.
#[derive(Clone, Debug, Default)]
pub struct EngineStatus {
    pub generation: u64,
    pub audio: AudioSnapshot,
    pub now_playing: NowPlaying,
}

/// Handle the main loop keeps to the engine thread.
pub struct AudioEngine {
    commands: mpsc::Sender<AudioCommand>,
    status: Arc<Mutex<EngineStatus>>,
    seen_generation: u64,
}

impl AudioEngine {
    /// Start the engine thread with an initialized codec. The I2S0
    /// peripheral and its four pins must have been moved out of
    /// `Peripherals` and never be used elsewhere: the engine re-creates the
    /// channel whenever the sample rate changes.
    pub fn start<I2C>(runtime: AudioRuntime<'static, I2C>) -> Result<Self>
    where
        I2C: I2c + Send + 'static,
        I2C::Error: core::fmt::Debug,
    {
        let (commands, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(EngineStatus {
            generation: 1,
            audio: runtime.snapshot(),
            now_playing: NowPlaying::default(),
        }));
        let shared = Arc::clone(&status);
        spawn_audio_thread(move || Engine::new(runtime, shared).run(&receiver))?;
        Ok(Self {
            commands,
            status,
            seen_generation: 0,
        })
    }

    /// `false` only if the engine thread is gone.
    pub fn send(&self, command: AudioCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    /// The engine's state, if it changed since the last call.
    pub fn poll(&mut self) -> Option<EngineStatus> {
        let status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if status.generation == self.seen_generation {
            return None;
        }
        self.seen_generation = status.generation;
        Some(status.clone())
    }

    #[must_use]
    pub fn current(&self) -> EngineStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stop all output (pausing a book) before the codec rail is cut.
    pub fn suspend(&self) {
        let (reply, done) = mpsc::sync_channel(1);
        if self.send(AudioCommand::Suspend(reply))
            && done.recv_timeout(Duration::from_secs(2)).is_err()
        {
            warn!("rustmix-wave=audio-engine status=suspend-timeout");
        }
    }

    /// Reprogram the codec after its rail was restored.
    pub fn restore(&self) -> Result<(), String> {
        let (reply, done) = mpsc::sync_channel(1);
        if !self.send(AudioCommand::Restore(reply)) {
            return Err("audio engine stopped".into());
        }
        done.recv_timeout(Duration::from_secs(3))
            .map_err(|_| "audio engine did not answer".to_string())?
    }
}

fn spawn_audio_thread<F>(body: F) -> Result<()>
where
    F: FnOnce() + Send + 'static,
{
    use esp_idf_svc::sys::{esp_pthread_get_default_config, esp_pthread_set_cfg};

    // Thread-local to this task, and only for its next spawn: restored
    // right after, like `runtime_worker` does.
    let mut cfg = unsafe { esp_pthread_get_default_config() };
    cfg.prio = AUDIO_THREAD_PRIORITY as _;
    cfg.pin_to_core = AUDIO_THREAD_CORE as _;
    cfg.thread_name = c"audio".as_ptr();
    let status = unsafe { esp_pthread_set_cfg(&cfg) };
    if status != 0 {
        warn!("rustmix-wave=audio-engine status=thread-cfg-failed error-code={status}");
    }
    let spawned = thread::Builder::new()
        .name("audio".into())
        .stack_size(AUDIO_THREAD_STACK_BYTES)
        .spawn(body);
    let restore = unsafe { esp_pthread_get_default_config() };
    unsafe { esp_pthread_set_cfg(&restore) };
    spawned.map(|_| ()).context("spawn audio thread")
}

/// Write PCM to the open channel; blocks while the DMA ring is full, which
/// is what paces playback.
fn write_i2s(i2s: &mut Option<(I2sDriver<'static, I2sTx>, u32)>, data: &[u8]) -> Result<()> {
    let Some((driver, _)) = i2s.as_mut() else {
        return Err(anyhow!("I2S channel not open"));
    };
    driver
        .write_all(data, TickType::new_millis(I2S_WRITE_TIMEOUT_MS).ticks())
        .map_err(|error| anyhow!("I2S TX write failed: {error:?}"))
}

/// Create the I2S0 TX channel at `sample_rate`, MCLK at 256× (the ratio the
/// codec is set up for).
fn open_i2s(sample_rate: u32) -> Result<I2sDriver<'static, I2sTx>> {
    let config = StdConfig::new(
        I2sChannelConfig::new()
            .auto_clear(true)
            .dma_buffer_count(DMA_BUFFERS)
            .frames_per_buffer(DMA_FRAMES_PER_BUFFER),
        StdClkConfig::new(sample_rate, ClockSource::default(), MclkMultiple::M256),
        StdSlotConfig::philips_slot_default(DataBitWidth::Bits16, SlotMode::Stereo),
        StdGpioConfig::default(),
    );
    // Safety: main moved I2S0 and these pins out of `Peripherals` for the
    // engine alone, and the engine drops its previous driver before making
    // a new one, so at most one instance of each exists at a time.
    let mut driver = unsafe {
        I2sDriver::<I2sTx>::new_std_tx(
            I2S0::steal(),
            &config,
            AnyIOPin::steal(AUDIO_BCLK_GPIO),
            AnyIOPin::steal(AUDIO_DOUT_GPIO),
            Some(AnyIOPin::steal(AUDIO_MCLK_GPIO)),
            AnyIOPin::steal(AUDIO_WS_GPIO),
        )
    }
    .map_err(|error| anyhow!("I2S setup at {sample_rate} Hz failed: {error:?}"))?;
    driver
        .tx_enable()
        .map_err(|error| anyhow!("I2S enable failed: {error:?}"))?;
    Ok(driver)
}

struct Engine<I2C> {
    runtime: AudioRuntime<'static, I2C>,
    i2s: Option<(I2sDriver<'static, I2sTx>, u32)>,
    chime: ChimeGenerator,
    book: Option<BookPlayback>,
    /// Kept after a book stops or fails, so the UI still shows where.
    now_playing: NowPlaying,
    status: Arc<Mutex<EngineStatus>>,
    last_publish: Instant,
    pcm: Box<[i16; MAX_FRAME_SAMPLES]>,
    out: Vec<u8>,
}

/// An audiobook loaded for playback: the open track and its decoder.
struct BookPlayback {
    book: Audiobook,
    track: usize,
    file: File,
    file_bytes: u64,
    info: Mp3StreamInfo,
    decoder: Mp3Decoder,
    input: Vec<u8>,
    start: usize,
    end: usize,
    eof: bool,
    /// File offset of `input[start]`: where decoding would resume.
    consumed_offset: u64,
    /// Time at `consumed_offset` when the track was opened or last sought,
    /// plus what has been decoded since.
    base_ms: u64,
    decoded_samples: u64,
    sample_rate: u32,
    paused: bool,
    /// Frames decoded in this track and the time spent decoding them, for
    /// the log: a frame lasts 26 ms at 44.1 kHz, decoding must stay well
    /// below that.
    frames: u64,
    decode_time: Duration,
}

impl BookPlayback {
    fn open(book: Audiobook, track: usize, byte_offset: u64) -> Result<Self> {
        let path = &book
            .tracks
            .get(track)
            .ok_or_else(|| anyhow!("track {track} out of range"))?
            .path;
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let file_bytes = file.metadata().map_or(0, |metadata| metadata.len());
        let info = probe(&mut file)?.ok_or_else(|| anyhow!("{} is not an MP3", path.display()))?;
        let decoder = Mp3Decoder::new().ok_or_else(|| anyhow!("MP3 decoder out of memory"))?;
        let mut playback = Self {
            book,
            track,
            file,
            file_bytes,
            info,
            decoder,
            input: vec![0; INPUT_BUFFER_BYTES],
            start: 0,
            end: 0,
            eof: false,
            consumed_offset: 0,
            base_ms: 0,
            decoded_samples: 0,
            sample_rate: info.sample_rate,
            paused: false,
            frames: 0,
            decode_time: Duration::ZERO,
        };
        playback.seek_to_byte(byte_offset.max(info.audio_start))?;
        Ok(playback)
    }

    fn seek_to_byte(&mut self, byte: u64) -> Result<()> {
        let byte = byte.min(self.file_bytes);
        self.file.seek(SeekFrom::Start(byte))?;
        self.start = 0;
        self.end = 0;
        self.eof = false;
        self.consumed_offset = byte;
        self.base_ms = self.info.ms_for_byte(byte, self.file_bytes);
        self.decoded_samples = 0;
        // The bit reservoir from before the jump belongs to other frames.
        self.decoder = Mp3Decoder::new().ok_or_else(|| anyhow!("MP3 decoder out of memory"))?;
        Ok(())
    }

    fn position_ms(&self) -> u64 {
        self.base_ms + self.decoded_samples * 1000 / u64::from(self.sample_rate.max(1))
    }

    fn log_stats(&self, event: &str) {
        let per_frame_us = self.decode_time.as_micros() / u128::from(self.frames.max(1));
        info!(
            "rustmix-wave=audiobook-stats event={event} track={} frames={} decode-us-per-frame={per_frame_us} position-ms={}",
            self.track + 1,
            self.frames,
            self.position_ms()
        );
    }

    fn duration_ms(&self) -> u64 {
        self.info.duration_ms(self.file_bytes)
    }

    fn position(&self) -> ListeningPosition {
        ListeningPosition {
            track: self.track,
            byte_offset: self.consumed_offset,
            position_ms: self.position_ms(),
        }
    }

    /// Top the input buffer up: move the unread tail to the front, read.
    fn fill(&mut self) -> Result<()> {
        if self.start > 0 {
            self.input.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        if self.end < self.input.len() && !self.eof {
            let read = self.file.read(&mut self.input[self.end..])?;
            if read == 0 {
                self.eof = true;
            }
            self.end += read;
        }
        Ok(())
    }
}

/// Probe the stream from the start of `file`, stepping over an ID3 tag
/// larger than the probe window.
fn probe(file: &mut File) -> Result<Option<Mp3StreamInfo>> {
    let mut head = vec![0_u8; MP3_PROBE_BYTES];
    let mut offset = 0_u64;
    for _ in 0..2 {
        file.seek(SeekFrom::Start(offset))?;
        let mut filled = 0;
        while filled < head.len() {
            let read = file.read(&mut head[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        match probe_mp3_at(&head[..filled], offset) {
            Mp3Probe::Found(info) => return Ok(Some(info)),
            Mp3Probe::SkipTo(next) => offset = next,
            Mp3Probe::NotMp3 => return Ok(None),
        }
    }
    Ok(None)
}

impl<I2C> Engine<I2C>
where
    I2C: I2c,
    I2C::Error: core::fmt::Debug,
{
    fn new(runtime: AudioRuntime<'static, I2C>, status: Arc<Mutex<EngineStatus>>) -> Self {
        Self {
            runtime,
            i2s: None,
            chime: ChimeGenerator::default(),
            book: None,
            now_playing: NowPlaying::default(),
            status,
            last_publish: Instant::now(),
            pcm: Box::new([0; MAX_FRAME_SAMPLES]),
            out: Vec::with_capacity(MAX_FRAME_SAMPLES * 2 * 2),
        }
    }

    fn run(mut self, commands: &mpsc::Receiver<AudioCommand>) {
        info!("rustmix-wave=audio-engine status=running core={AUDIO_THREAD_CORE} priority={AUDIO_THREAD_PRIORITY}");
        loop {
            let next = if self.streaming() {
                commands.try_recv().map_err(|error| match error {
                    mpsc::TryRecvError::Empty => None,
                    mpsc::TryRecvError::Disconnected => Some(()),
                })
            } else {
                commands
                    .recv_timeout(IDLE_WAIT)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => None,
                        mpsc::RecvTimeoutError::Disconnected => Some(()),
                    })
            };
            match next {
                Ok(command) => {
                    self.handle(command);
                    self.publish();
                    continue;
                }
                Err(Some(())) => break,
                Err(None) => {}
            }
            if self.chime.is_playing() {
                if let Err(error) = self.pump_chime() {
                    self.fail(&error);
                }
            } else if self.book.as_ref().is_some_and(|book| !book.paused) {
                if let Err(error) = self.pump_book() {
                    self.fail_book(&error);
                }
            }
            if self.last_publish.elapsed() >= POSITION_PUBLISH_INTERVAL && self.streaming() {
                self.publish();
            }
        }
        info!("rustmix-wave=audio-engine status=stopped");
    }

    fn streaming(&self) -> bool {
        self.chime.is_playing() || self.book.as_ref().is_some_and(|book| !book.paused)
    }

    fn handle(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::Ui(request) => {
                if let Err(error) = self.apply_ui(request) {
                    warn!("rustmix-wave=audio-event outcome=request-failed request={request:?} error={error:#}");
                    self.fail(&error);
                }
            }
            AudioCommand::Player(command) => {
                if let Err(error) = self.apply_player(command) {
                    self.fail_book(&error);
                }
            }
            AudioCommand::Suspend(done) => {
                self.chime.stop();
                self.pause_book();
                let _ = self.runtime.end_output();
                self.i2s = None;
                info!("rustmix-wave=audio-engine status=suspended");
                let _ = done.send(());
            }
            AudioCommand::Restore(done) => {
                let outcome = self
                    .runtime
                    .reinit_after_rail_restore(&mut FreeRtos)
                    .map_err(|error| format!("{error:#}"));
                if let Err(error) = &outcome {
                    self.runtime.record_failure(error.clone());
                }
                let _ = done.send(outcome);
            }
        }
    }

    fn apply_ui(&mut self, request: AudioUiRequest) -> Result<()> {
        let volume = self.runtime.snapshot().volume_percent;
        match request {
            AudioUiRequest::PlayTestChime => {
                self.pause_book();
                self.ensure_i2s(AUDIO_SAMPLE_RATE_HZ)?;
                self.chime.start_test_once();
                self.runtime
                    .begin_output(AudioPlaybackState::PlayingTestTone)?;
                info!("rustmix-wave=audio-event outcome=test-tone-start");
            }
            AudioUiRequest::StopPlayback => {
                self.chime.stop();
                self.pause_book();
                self.runtime.end_output()?;
                self.i2s = None;
                info!("rustmix-wave=audio-event outcome=playback-stop");
            }
            AudioUiRequest::VolumeUp => {
                self.runtime
                    .set_volume(volume.saturating_add(AUDIO_VOLUME_STEP_PERCENT))?;
            }
            AudioUiRequest::VolumeDown => {
                self.runtime
                    .set_volume(volume.saturating_sub(AUDIO_VOLUME_STEP_PERCENT))?;
            }
            AudioUiRequest::ToggleMute => {
                let muted = !self.runtime.snapshot().muted;
                let playing = if self.chime.is_playing() {
                    Some(AudioPlaybackState::PlayingTestTone)
                } else if self.streaming() {
                    Some(AudioPlaybackState::PlayingAudiobook)
                } else {
                    None
                };
                self.runtime.set_muted(muted, playing)?;
            }
        }
        Ok(())
    }

    fn apply_player(&mut self, command: PlayerCommand) -> Result<()> {
        match command {
            PlayerCommand::Play { book, position } => {
                self.chime.stop();
                let mut position = position;
                if position.track >= book.tracks.len() {
                    position = ListeningPosition::default();
                }
                // A book played to its end starts over.
                let at_end = position.track + 1 == book.tracks.len()
                    && position.byte_offset >= book.tracks[position.track].size_bytes;
                if at_end {
                    position = ListeningPosition::default();
                }
                self.now_playing.error = None;
                let playback = BookPlayback::open(book, position.track, position.byte_offset)?;
                info!(
                    "rustmix-wave=audiobook status=open title={:?} track={} of={} byte={} rate={} channels={} kbps={} duration-ms={}",
                    playback.book.title,
                    playback.track + 1,
                    playback.book.tracks.len(),
                    playback.consumed_offset,
                    playback.info.sample_rate,
                    playback.info.channels,
                    playback.info.bitrate_kbps,
                    playback.duration_ms()
                );
                self.start_book(playback)?;
            }
            PlayerCommand::TogglePause => match self.book.as_ref().map(|book| book.paused) {
                Some(false) => self.pause_book(),
                Some(true) => {
                    let rate = self.book.as_ref().map_or(0, |book| book.sample_rate);
                    self.ensure_i2s(rate)?;
                    if let Some(book) = self.book.as_mut() {
                        book.paused = false;
                    }
                    self.runtime
                        .begin_output(AudioPlaybackState::PlayingAudiobook)?;
                    info!("rustmix-wave=audiobook status=resumed");
                }
                None => {}
            },
            PlayerCommand::SeekBy { seconds } => {
                if let Some(book) = self.book.as_mut() {
                    let duration = book.duration_ms();
                    let target = book
                        .position_ms()
                        .saturating_add_signed(i64::from(seconds) * 1000)
                        .min(duration.saturating_sub(1_000));
                    let byte = book.info.byte_for_ms(target, book.file_bytes);
                    book.seek_to_byte(byte)?;
                    info!("rustmix-wave=audiobook status=seek seconds={seconds} target-ms={target} byte={byte}");
                }
            }
            PlayerCommand::SkipTrack { forward } => {
                let Some(book) = self.book.as_ref() else {
                    return Ok(());
                };
                let current = book.track;
                let target = if forward {
                    current + 1
                } else if book.position_ms() > RESTART_TRACK_AFTER_MS || current == 0 {
                    current
                } else {
                    current - 1
                };
                self.switch_track(target)?;
            }
            PlayerCommand::Stop => {
                if let Some(book) = self.book.take() {
                    book.log_stats("stop");
                    self.now_playing = self.describe(&book, PlayerState::Stopped);
                }
                self.runtime.end_output()?;
                self.i2s = None;
                info!("rustmix-wave=audiobook status=stopped");
            }
        }
        Ok(())
    }

    /// Start streaming a freshly opened book.
    fn start_book(&mut self, playback: BookPlayback) -> Result<()> {
        let rate = playback.sample_rate;
        self.book = Some(playback);
        self.ensure_i2s(rate)?;
        self.runtime
            .begin_output(AudioPlaybackState::PlayingAudiobook)?;
        Ok(())
    }

    /// Move to track `target` of the loaded book, from its start; past the
    /// last track the book is finished.
    fn switch_track(&mut self, target: usize) -> Result<()> {
        let Some(book) = self.book.take() else {
            return Ok(());
        };
        book.log_stats("track-change");
        let paused = book.paused;
        if target >= book.book.tracks.len() {
            let mut finished = self.describe(&book, PlayerState::Finished);
            finished.position = ListeningPosition {
                track: book.track,
                byte_offset: book.file_bytes,
                position_ms: book.duration_ms(),
            };
            self.now_playing = finished;
            self.runtime.end_output()?;
            self.i2s = None;
            info!(
                "rustmix-wave=audiobook status=finished title={:?}",
                book.book.title
            );
            return Ok(());
        }
        let mut next = BookPlayback::open(book.book, target, 0)?;
        next.paused = paused;
        info!(
            "rustmix-wave=audiobook status=track track={} of={} rate={}",
            target + 1,
            next.book.tracks.len(),
            next.sample_rate
        );
        let rate = next.sample_rate;
        self.book = Some(next);
        if !paused {
            if self.i2s.as_ref().map(|(_, current)| *current) != Some(rate) {
                // Amplifier off across the channel swap: no pop.
                self.runtime.end_output()?;
                self.ensure_i2s(rate)?;
                self.runtime
                    .begin_output(AudioPlaybackState::PlayingAudiobook)?;
            }
        }
        Ok(())
    }

    fn pause_book(&mut self) {
        let Some(book) = self.book.as_mut() else {
            return;
        };
        if book.paused {
            return;
        }
        book.paused = true;
        book.log_stats("pause");
        let _ = self.runtime.end_output();
        // Dropping the channel releases its power-management lock, so the
        // CPU can sleep while paused.
        self.i2s = None;
        info!("rustmix-wave=audiobook status=paused");
    }

    fn ensure_i2s(&mut self, sample_rate: u32) -> Result<()> {
        if self.i2s.as_ref().map(|(_, rate)| *rate) == Some(sample_rate) {
            return Ok(());
        }
        self.i2s = None;
        let driver = open_i2s(sample_rate)?;
        self.i2s = Some((driver, sample_rate));
        Ok(())
    }

    fn pump_chime(&mut self) -> Result<()> {
        let snapshot = self.runtime.snapshot();
        let mut bytes = [0_u8; PCM_CHUNK_BYTES];
        let completed =
            self.chime
                .fill_stereo_pcm(&mut bytes, snapshot.volume_percent, snapshot.muted);
        write_i2s(&mut self.i2s, &bytes)?;
        if completed {
            self.runtime.end_output()?;
            self.i2s = None;
            self.publish();
        }
        Ok(())
    }

    /// Decode and play one frame of the loaded book.
    fn pump_book(&mut self) -> Result<()> {
        let (step, track_ended) = {
            let Some(book) = self.book.as_mut() else {
                return Ok(());
            };
            if book.end - book.start < MAX_FRAME_BYTES * 2 && !book.eof {
                book.fill()?;
            }
            let mut window = &book.input[book.start..book.end];
            let before = window.len();
            let started = Instant::now();
            let step = book.decoder.decode(&mut window, &mut self.pcm);
            if matches!(step, DecodeStep::Frame(_)) {
                book.frames += 1;
                book.decode_time += started.elapsed();
            }
            let consumed = before - window.len();
            book.start += consumed;
            book.consumed_offset += consumed as u64;
            let track_ended = step == DecodeStep::NeedMoreInput
                && book.eof
                && book.end - book.start < MAX_FRAME_BYTES;
            if step == DecodeStep::NeedMoreInput && !track_ended {
                book.fill()?;
            }
            if let DecodeStep::Frame(frame) = step {
                if frame.sample_rate != book.sample_rate && frame.sample_rate > 0 {
                    // A track that changes rate mid-stream: follow it.
                    book.base_ms = book.position_ms();
                    book.decoded_samples = 0;
                    book.sample_rate = frame.sample_rate;
                }
                book.decoded_samples += (frame.samples / usize::from(frame.channels)) as u64;
            }
            (step, track_ended)
        };
        if track_ended {
            let next = self.book.as_ref().map_or(0, |book| book.track + 1);
            self.switch_track(next)?;
            self.publish();
            return Ok(());
        }
        let DecodeStep::Frame(frame) = step else {
            return Ok(());
        };
        self.out.clear();
        for sample in &self.pcm[..frame.samples] {
            let bytes = sample.to_le_bytes();
            self.out.extend_from_slice(&bytes);
            if frame.channels == 1 {
                // Mono: the same sample on both slots.
                self.out.extend_from_slice(&bytes);
            }
        }
        self.ensure_i2s(frame.sample_rate)?;
        write_i2s(&mut self.i2s, &self.out)
    }

    fn describe(&self, book: &BookPlayback, state: PlayerState) -> NowPlaying {
        let track = &book.book.tracks[book.track];
        NowPlaying {
            key: book.book.key.clone(),
            title: book.book.title.clone(),
            track: book.track,
            track_count: book.book.tracks.len(),
            track_title: track.title.clone(),
            position: book.position(),
            duration_ms: book.duration_ms(),
            state,
            error: None,
        }
    }

    /// A failure outside book playback (test tone, codec): silence.
    fn fail(&mut self, error: &anyhow::Error) {
        warn!("rustmix-wave=audio-engine status=error error={error:#}");
        self.chime.stop();
        self.i2s = None;
        self.runtime.record_failure(format!("{error:#}"));
        self.publish();
    }

    /// A failure while playing a book: keep its position, show the error.
    fn fail_book(&mut self, error: &anyhow::Error) {
        warn!("rustmix-wave=audiobook status=error error={error:#}");
        if let Some(book) = self.book.take() {
            let mut failed = self.describe(&book, PlayerState::Error);
            failed.error = Some(format!("{error:#}"));
            self.now_playing = failed;
        } else {
            self.now_playing.state = PlayerState::Error;
            self.now_playing.error = Some(format!("{error:#}"));
        }
        let _ = self.runtime.end_output();
        self.i2s = None;
        self.publish();
    }

    fn publish(&mut self) {
        let now_playing = match self.book.as_ref() {
            Some(book) => self.describe(
                book,
                if book.paused {
                    PlayerState::Paused
                } else {
                    PlayerState::Playing
                },
            ),
            None => self.now_playing.clone(),
        };
        let audio = self.runtime.snapshot();
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        status.generation = status.generation.wrapping_add(1);
        status.audio = audio;
        status.now_playing = now_playing;
        drop(status);
        self.last_publish = Instant::now();
    }
}
