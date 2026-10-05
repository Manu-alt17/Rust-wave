//! Input timing diagnostic (temporary).
//!
//! Times every phase a key goes through, from the contact closing to the
//! panel finishing its refresh, so the waits the firmware assumes (key
//! sampling period, debounce, release debounce, BUSY polling) can be checked
//! against the real hardware. It changes no behaviour: it only takes
//! timestamps, and writes its report while the device is idle -- never
//! while a key is being handled, since a line costs ~87 us per character on
//! UART0 and would distort the very timings being measured.
//!
//! Report lines, all times in milliseconds:
//!
//! - `itp` one handled key: `t` is when the input thread first read the key
//!   down (since boot), then how long each phase took: `deb` debounce until
//!   queued, `wait` in the queue, `wake` panel wake-up (only when it was
//!   asleep), `snap` RTC and PMIC read, `apply` state change, `render`
//!   drawing the frame in memory, `spi` sending it, `busy` the panel's own
//!   refresh, `tail` what is left, and `total` from dequeue to done.
//! - `itx` a key the queue dropped because it was full.
//! - `ite` raw level changes of the four keys, sampled every millisecond:
//!   `U`p, `D`own, `S`elect, `B`oot followed by the level (0 = pressed).
//! - `its` median / minimum / maximum of the main phases since the last one.
//! - `itq` the panel's analog supply was switched off after sitting idle.
//!
//! [`ENABLED`] is `false` in the repository; a unit test keeps it so, which
//! makes the release validation fail for a tree that still has it on.

use std::{
    collections::VecDeque,
    fmt::Write as _,
    sync::{Mutex, PoisonError},
};

use crate::boot_profile::now_us;

/// Whether the diagnostic records and reports anything. Every hook checks
/// this constant first, so with `false` the compiler removes them.
pub const ENABLED: bool = false;

/// SD copy of the report, for runs made on battery with no serial monitor.
pub const REPORT_PATH: &str = "/sdcard/RUSTMIX/INTIMING.TXT";

/// Keys sampled by the firmware's 1 ms level sampler, as bits of the mask
/// passed to [`sample_levels`]. A set bit is a high level (key released).
pub const KEY_UP_BIT: u8 = 1 << 0;
pub const KEY_DOWN_BIT: u8 = 1 << 1;
pub const KEY_SELECT_BIT: u8 = 1 << 2;
pub const KEY_BOOT_BIT: u8 = 1 << 3;
const KEY_BITS: [(u8, char); 4] = [
    (KEY_UP_BIT, 'U'),
    (KEY_DOWN_BIT, 'D'),
    (KEY_SELECT_BIT, 'S'),
    (KEY_BOOT_BIT, 'B'),
];

/// Level changes kept between two reports; older ones are dropped first.
const MAX_EDGES: usize = 1024;
/// Report lines kept while waiting for an idle moment to print them.
const MAX_LINES: usize = 600;
/// Level changes written on one `ite` line.
const EDGES_PER_LINE: usize = 6;

/// When the input thread saw a key and when it queued the event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventTiming {
    /// First sample that read the key down.
    pub seen_us: i64,
    /// The debounced event entered the queue.
    pub queued_us: i64,
}

/// A point in the handling of one key, in the order they happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// The panel was asleep and has been woken.
    Woke,
    /// RTC and PMIC have been read.
    Snapshot,
    /// The state change is applied; the refresh comes next.
    Applied,
    /// The frame is drawn in memory.
    Rendered,
    /// The panel driver starts sending the frame.
    PanelStart,
    /// The frame is in the controller's RAM.
    SpiDone,
    /// The controller released BUSY: the glass shows the frame.
    BusyDone,
}

#[derive(Clone, Debug, Default)]
struct Record {
    seq: u32,
    key: &'static str,
    timing: EventTiming,
    dequeued_us: i64,
    queue_len: usize,
    route_before: &'static str,
    woke_us: Option<i64>,
    snapshot_us: Option<i64>,
    applied_us: Option<i64>,
    rendered_us: Option<i64>,
    panel_start_us: Option<i64>,
    spi_done_us: Option<i64>,
    busy_done_us: Option<i64>,
    refreshes: u8,
    plan: &'static str,
}

/// Durations of one finished key, kept for the `its` summary.
#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    total: i64,
    busy: Option<i64>,
    spi: Option<i64>,
    render: Option<i64>,
    snap: Option<i64>,
    deb: i64,
    wait: i64,
}

/// Everything the main loop and the panel driver record. Pure: every method
/// takes its timestamp, so it is tested on the host.
#[derive(Debug, Default)]
pub struct Recorder {
    next_seq: u32,
    open: Option<Record>,
    lines: VecDeque<String>,
    lines_lost: u32,
    sd_backlog: Vec<String>,
    samples: Vec<Sample>,
}

impl Recorder {
    /// A key left the queue. A key still open (its handler ended early) is
    /// closed first.
    pub fn begin(
        &mut self,
        key: &'static str,
        timing: EventTiming,
        queue_len: usize,
        route_before: &'static str,
        at_us: i64,
    ) {
        if self.open.is_some() {
            self.end("?", at_us);
        }
        self.next_seq += 1;
        self.open = Some(Record {
            seq: self.next_seq,
            key,
            timing,
            dequeued_us: at_us,
            queue_len,
            route_before,
            plan: "none",
            ..Record::default()
        });
    }

    /// Record `phase` for the key being handled. Only the first refresh of
    /// a key is timed; later ones are counted.
    pub fn mark(&mut self, phase: Phase, at_us: i64) {
        let Some(record) = self.open.as_mut() else {
            return;
        };
        let slot = match phase {
            Phase::Woke => &mut record.woke_us,
            Phase::Snapshot => &mut record.snapshot_us,
            Phase::Applied => &mut record.applied_us,
            Phase::Rendered => &mut record.rendered_us,
            Phase::PanelStart => {
                record.refreshes = record.refreshes.saturating_add(1);
                &mut record.panel_start_us
            }
            Phase::SpiDone => &mut record.spi_done_us,
            Phase::BusyDone => &mut record.busy_done_us,
        };
        if slot.is_none() {
            *slot = Some(at_us);
        }
    }

    /// Kind of refresh the key caused (`partial`, `partial-keep-on`,
    /// `global`, `global-fast`).
    pub fn note_plan(&mut self, plan: &'static str) {
        if let Some(record) = self.open.as_mut() {
            if record.plan == "none" {
                record.plan = plan;
            }
        }
    }

    /// The key is fully handled.
    pub fn end(&mut self, route_after: &'static str, at_us: i64) {
        let Some(record) = self.open.take() else {
            return;
        };
        let after_wake = record.woke_us.unwrap_or(record.dequeued_us);
        let snap = record.snapshot_us.map(|at| at - after_wake);
        let apply = match (record.snapshot_us, record.applied_us) {
            (Some(from), Some(to)) => Some(to - from),
            _ => None,
        };
        let render_from = record
            .applied_us
            .or(record.snapshot_us)
            .unwrap_or(after_wake);
        let render = record.rendered_us.map(|at| at - render_from);
        let spi = match (record.rendered_us, record.spi_done_us) {
            (Some(from), Some(to)) => Some(to - from),
            _ => None,
        };
        let busy = match (record.spi_done_us, record.busy_done_us) {
            (Some(from), Some(to)) => Some(to - from),
            _ => None,
        };
        let tail = record.busy_done_us.map(|at| at_us - at);
        let total = at_us - record.dequeued_us;
        let deb = record.timing.queued_us - record.timing.seen_us;
        let wait = record.dequeued_us - record.timing.queued_us;

        let mut line = format!(
            "itp n={} key={} t={} deb={} wait={}",
            record.seq,
            record.key,
            ms(record.timing.seen_us),
            ms(deb),
            ms(wait),
        );
        if let Some(woke) = record.woke_us {
            let _ = write!(line, " wake={}", ms(woke - record.dequeued_us));
        }
        let _ = write!(
            line,
            " snap={} apply={} render={} spi={} busy={} tail={} total={} q={} plan={} ref={} route={}>{}",
            optional_ms(snap),
            optional_ms(apply),
            optional_ms(render),
            optional_ms(spi),
            optional_ms(busy),
            optional_ms(tail),
            ms(total),
            record.queue_len,
            record.plan,
            record.refreshes,
            record.route_before,
            route_after,
        );
        self.push_line(line);
        self.samples.push(Sample {
            total,
            busy,
            spi,
            render,
            snap,
            deb,
            wait,
        });
    }

    /// A free-form line (`itq …`) for something measured outside a key.
    pub fn note(&mut self, line: String) {
        self.push_line(line);
    }

    /// A key the queue dropped to make room for a newer one.
    pub fn note_dropped(&mut self, key: &'static str, timing: EventTiming) {
        self.push_line(format!(
            "itx key={key} t={} deb={}",
            ms(timing.seen_us),
            ms(timing.queued_us - timing.seen_us)
        ));
    }

    /// Whether a key is being handled right now.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Turn the level changes sampled since the last report into `ite`
    /// lines, and close the batch with an `its` summary.
    pub fn add_edges_and_summary(&mut self, edges: &[Edge], edges_lost: u32) {
        for chunk in edges.chunks(EDGES_PER_LINE) {
            let mut line = String::from("ite");
            for edge in chunk {
                let _ = write!(
                    line,
                    " {} {}{}",
                    ms(edge.at_us),
                    edge.key,
                    u8::from(edge.high)
                );
            }
            self.push_line(line);
        }
        if edges_lost > 0 {
            self.push_line(format!("ite lost={edges_lost}"));
        }
        if self.samples.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut self.samples);
        let line = format!(
            "its n={} total={} busy={} spi={} render={} snap={} deb={} wait={} lines-lost={}",
            samples.len(),
            spread(samples.iter().map(|sample| Some(sample.total))),
            spread(samples.iter().map(|sample| sample.busy)),
            spread(samples.iter().map(|sample| sample.spi)),
            spread(samples.iter().map(|sample| sample.render)),
            spread(samples.iter().map(|sample| sample.snap)),
            spread(samples.iter().map(|sample| Some(sample.deb))),
            spread(samples.iter().map(|sample| Some(sample.wait))),
            self.lines_lost,
        );
        self.lines_lost = 0;
        self.push_line(line);
    }

    /// Whether there are finished keys not yet summarised.
    #[must_use]
    pub fn has_unsummarised_keys(&self) -> bool {
        !self.samples.is_empty()
    }

    /// Next report line to print, oldest first.
    pub fn next_line(&mut self) -> Option<String> {
        let line = self.lines.pop_front()?;
        self.sd_backlog.push(line.clone());
        Some(line)
    }

    /// The printed lines not yet written to SD, once nothing is left to
    /// print: one write per burst of keys instead of one per line.
    pub fn take_sd_batch(&mut self) -> Option<String> {
        if !self.lines.is_empty() || self.sd_backlog.is_empty() {
            return None;
        }
        let mut text = String::new();
        for line in self.sd_backlog.drain(..) {
            text.push_str(&line);
            text.push('\n');
        }
        Some(text)
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() >= MAX_LINES {
            self.lines.pop_front();
            self.lines_lost += 1;
        }
        self.lines.push_back(line);
    }
}

/// One level change of one key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Edge {
    pub at_us: i64,
    /// `U`, `D`, `S` or `B`.
    pub key: char,
    /// New level; keys are active low, so `false` is "pressed".
    pub high: bool,
}

/// Level changes seen by the 1 ms sampler since the last report.
#[derive(Debug, Default)]
pub struct EdgeLog {
    last_mask: Option<u8>,
    edges: VecDeque<Edge>,
    lost: u32,
}

impl EdgeLog {
    /// Feed one sample of the four key levels.
    pub fn sample(&mut self, mask: u8, at_us: i64) {
        let Some(previous) = self.last_mask.replace(mask) else {
            // Reserve once, so the timer task never allocates afterwards.
            self.edges.reserve(MAX_EDGES);
            return;
        };
        let changed = previous ^ mask;
        if changed == 0 {
            return;
        }
        for (bit, key) in KEY_BITS {
            if changed & bit == 0 {
                continue;
            }
            if self.edges.len() >= MAX_EDGES {
                self.edges.pop_front();
                self.lost += 1;
            }
            self.edges.push_back(Edge {
                at_us,
                key,
                high: mask & bit != 0,
            });
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Take every recorded change, and how many were lost to the cap.
    pub fn take(&mut self) -> (Vec<Edge>, u32) {
        let edges = self.edges.drain(..).collect();
        (edges, std::mem::take(&mut self.lost))
    }
}

fn ms(us: i64) -> String {
    let tenths = (us + if us >= 0 { 50 } else { -50 }) / 100;
    let sign = if tenths < 0 { "-" } else { "" };
    let tenths = tenths.abs();
    format!("{sign}{}.{}", tenths / 10, tenths % 10)
}

fn optional_ms(us: Option<i64>) -> String {
    us.map_or_else(|| "-".to_string(), ms)
}

/// `median/min/max` of the values that are present, or `-`.
fn spread(values: impl Iterator<Item = Option<i64>>) -> String {
    let mut values: Vec<i64> = values.flatten().collect();
    if values.is_empty() {
        return "-".to_string();
    }
    values.sort_unstable();
    let median = values[values.len() / 2];
    format!(
        "{}/{}/{}",
        ms(median),
        ms(values[0]),
        ms(values[values.len() - 1])
    )
}

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);
static EDGES: Mutex<Option<EdgeLog>> = Mutex::new(None);

fn with_recorder<T>(action: impl FnOnce(&mut Recorder) -> T) -> T {
    let mut guard = RECORDER.lock().unwrap_or_else(PoisonError::into_inner);
    action(guard.get_or_insert_with(Recorder::default))
}

/// A key left the queue and its handling starts now.
pub fn begin(key: &'static str, timing: EventTiming, queue_len: usize, route_before: &'static str) {
    if ENABLED {
        let at = now_us();
        with_recorder(|recorder| recorder.begin(key, timing, queue_len, route_before, at));
    }
}

/// Timestamp `phase` of the key being handled, if any.
pub fn mark(phase: Phase) {
    if ENABLED {
        let at = now_us();
        with_recorder(|recorder| recorder.mark(phase, at));
    }
}

pub fn note_plan(plan: &'static str) {
    if ENABLED {
        with_recorder(|recorder| recorder.note_plan(plan));
    }
}

/// The key is fully handled.
pub fn end(route_after: &'static str) {
    if ENABLED {
        let at = now_us();
        with_recorder(|recorder| recorder.end(route_after, at));
    }
}

pub fn note_dropped(key: &'static str, timing: EventTiming) {
    if ENABLED {
        with_recorder(|recorder| recorder.note_dropped(key, timing));
    }
}

/// Add an `itq` line: the analog supply was switched off at `at_us`, which
/// took `took_us`.
pub fn note_analog_off(at_us: i64, took_us: i64) {
    if ENABLED {
        with_recorder(|recorder| {
            recorder.note(format!(
                "itq analog-off t={} took={}",
                ms(at_us),
                ms(took_us)
            ));
        });
    }
}

/// One sample of the key levels, from the firmware's 1 ms timer. Never
/// blocks: a sample that finds the log busy is skipped, and the change it
/// would have seen is picked up one millisecond later.
pub fn sample_levels(mask: u8) {
    if !ENABLED {
        return;
    }
    let at = now_us();
    if let Ok(mut guard) = EDGES.try_lock() {
        guard.get_or_insert_with(EdgeLog::default).sample(mask, at);
    }
}

/// Next report line to print. Call only while the device is idle: the
/// first call after a burst of keys also adds the level changes and the
/// summary of that burst.
#[must_use]
pub fn next_report_line() -> Option<String> {
    if !ENABLED {
        return None;
    }
    with_recorder(|recorder| {
        if let Some(line) = recorder.next_line() {
            return Some(line);
        }
        if recorder.is_open() {
            return None;
        }
        let (edges, lost) = {
            let mut guard = EDGES.lock().unwrap_or_else(PoisonError::into_inner);
            let log = guard.get_or_insert_with(EdgeLog::default);
            if log.is_empty() && !recorder.has_unsummarised_keys() {
                return None;
            }
            log.take()
        };
        recorder.add_edges_and_summary(&edges, lost);
        recorder.next_line()
    })
}

/// Printed lines to append to [`REPORT_PATH`], once the report is out.
#[must_use]
pub fn take_sd_batch() -> Option<String> {
    if !ENABLED {
        return None;
    }
    with_recorder(Recorder::take_sd_batch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(seen_us: i64, queued_us: i64) -> EventTiming {
        EventTiming { seen_us, queued_us }
    }

    #[test]
    fn the_diagnostic_is_off_in_a_tree_that_can_be_released() {
        assert!(
            !ENABLED,
            "input_timing::ENABLED is on: this tree is a diagnostic build, set it back to false before releasing"
        );
    }

    #[test]
    fn a_key_reports_every_phase_as_a_duration() {
        let mut recorder = Recorder::default();
        recorder.begin("down", timing(1_000_000, 1_010_400), 1, "home", 1_010_900);
        recorder.mark(Phase::Snapshot, 1_013_000);
        recorder.mark(Phase::Applied, 1_013_200);
        recorder.note_plan("partial");
        recorder.mark(Phase::Rendered, 1_027_800);
        recorder.mark(Phase::PanelStart, 1_027_900);
        recorder.mark(Phase::SpiDone, 1_049_000);
        recorder.mark(Phase::BusyDone, 1_546_000);
        recorder.end("home", 1_546_400);
        assert_eq!(
            recorder.next_line().as_deref(),
            Some(
                "itp n=1 key=down t=1000.0 deb=10.4 wait=0.5 snap=2.1 apply=0.2 render=14.6 spi=21.2 busy=497.0 tail=0.4 total=535.5 q=1 plan=partial ref=1 route=home>home"
            )
        );
        assert_eq!(recorder.next_line(), None);
    }

    #[test]
    fn a_wake_up_is_reported_and_kept_out_of_the_snapshot_time() {
        let mut recorder = Recorder::default();
        recorder.begin("up", timing(0, 10_000), 0, "library", 10_000);
        recorder.mark(Phase::Woke, 190_000);
        recorder.mark(Phase::Snapshot, 192_000);
        recorder.end("library", 200_000);
        let line = recorder.next_line().unwrap();
        assert!(line.contains(" wake=180.0 snap=2.0 "), "{line}");
        assert!(line.contains(" render=- spi=- busy=- tail=- "), "{line}");
        assert!(line.contains(" plan=none ref=0 "), "{line}");
    }

    #[test]
    fn only_the_first_refresh_of_a_key_is_timed_and_the_rest_counted() {
        let mut recorder = Recorder::default();
        recorder.begin("select", timing(0, 0), 0, "upload", 0);
        recorder.mark(Phase::PanelStart, 1_000);
        recorder.mark(Phase::SpiDone, 21_000);
        recorder.mark(Phase::BusyDone, 500_000);
        recorder.mark(Phase::PanelStart, 600_000);
        recorder.mark(Phase::SpiDone, 620_000);
        recorder.mark(Phase::BusyDone, 1_100_000);
        recorder.end("usb", 1_100_000);
        let line = recorder.next_line().unwrap();
        assert!(line.contains(" ref=2 "), "{line}");
        assert!(line.contains(" tail=600.0 "), "{line}");
    }

    #[test]
    fn a_key_whose_handler_ended_early_is_closed_by_the_next_one() {
        let mut recorder = Recorder::default();
        recorder.begin("back", timing(0, 0), 0, "home", 0);
        recorder.begin("down", timing(50_000, 60_000), 0, "home", 60_000);
        recorder.end("home", 600_000);
        let first = recorder.next_line().unwrap();
        assert!(first.starts_with("itp n=1 key=back "), "{first}");
        assert!(first.ends_with(" route=home>?"), "{first}");
        let second = recorder.next_line().unwrap();
        assert!(second.starts_with("itp n=2 key=down "), "{second}");
    }

    #[test]
    fn marks_without_a_key_being_handled_are_ignored() {
        let mut recorder = Recorder::default();
        recorder.mark(Phase::BusyDone, 5);
        recorder.note_plan("global");
        recorder.end("home", 10);
        assert_eq!(recorder.next_line(), None);
    }

    #[test]
    fn a_dropped_key_gets_its_own_line() {
        let mut recorder = Recorder::default();
        recorder.note_dropped("down", timing(2_000_000, 2_011_000));
        assert_eq!(
            recorder.next_line().as_deref(),
            Some("itx key=down t=2000.0 deb=11.0")
        );
    }

    #[test]
    fn the_summary_gives_median_minimum_and_maximum() {
        let mut recorder = Recorder::default();
        for (index, busy) in [500_000_i64, 480_000, 520_000].into_iter().enumerate() {
            let start = index as i64 * 1_000_000;
            recorder.begin(
                "down",
                timing(start, start + 10_000),
                0,
                "home",
                start + 10_000,
            );
            recorder.mark(Phase::Rendered, start + 20_000);
            recorder.mark(Phase::SpiDone, start + 40_000);
            recorder.mark(Phase::BusyDone, start + 40_000 + busy);
            recorder.end("home", start + 40_000 + busy);
        }
        recorder.add_edges_and_summary(&[], 0);
        let mut last = String::new();
        while let Some(line) = recorder.next_line() {
            last = line;
        }
        assert_eq!(
            last,
            "its n=3 total=530.0/510.0/550.0 busy=500.0/480.0/520.0 spi=20.0/20.0/20.0 render=10.0/10.0/10.0 snap=- deb=10.0/10.0/10.0 wait=0.0/0.0/0.0 lines-lost=0"
        );
        // Summarised once: nothing new to say until another key is handled.
        recorder.add_edges_and_summary(&[], 0);
        assert_eq!(recorder.next_line(), None);
    }

    #[test]
    fn level_changes_are_logged_per_key_with_their_new_level() {
        let mut log = EdgeLog::default();
        let released = KEY_UP_BIT | KEY_DOWN_BIT | KEY_SELECT_BIT | KEY_BOOT_BIT;
        log.sample(released, 0);
        log.sample(released, 1_000);
        log.sample(released & !KEY_DOWN_BIT, 2_000);
        log.sample(released & !KEY_DOWN_BIT, 3_000);
        log.sample(released & !KEY_SELECT_BIT, 90_000);
        let (edges, lost) = log.take();
        assert_eq!(lost, 0);
        assert_eq!(
            edges,
            vec![
                Edge {
                    at_us: 2_000,
                    key: 'D',
                    high: false
                },
                Edge {
                    at_us: 90_000,
                    key: 'D',
                    high: true
                },
                Edge {
                    at_us: 90_000,
                    key: 'S',
                    high: false
                },
            ]
        );
        assert!(log.is_empty());
    }

    #[test]
    fn level_changes_are_written_a_few_per_line() {
        let mut recorder = Recorder::default();
        let edges: Vec<Edge> = (0..7)
            .map(|index| Edge {
                at_us: 1_000_000 + index * 1_500,
                key: 'U',
                high: index % 2 == 1,
            })
            .collect();
        recorder.add_edges_and_summary(&edges, 3);
        assert_eq!(
            recorder.next_line().as_deref(),
            Some("ite 1000.0 U0 1001.5 U1 1003.0 U0 1004.5 U1 1006.0 U0 1007.5 U1")
        );
        assert_eq!(recorder.next_line().as_deref(), Some("ite 1009.0 U0"));
        assert_eq!(recorder.next_line().as_deref(), Some("ite lost=3"));
        assert_eq!(recorder.next_line(), None);
    }

    #[test]
    fn the_oldest_changes_are_dropped_past_the_cap() {
        let mut log = EdgeLog::default();
        log.sample(0, 0);
        for index in 0..(MAX_EDGES as i64 + 5) {
            let mask = if index % 2 == 0 { KEY_UP_BIT } else { 0 };
            log.sample(mask, index + 1);
        }
        let (edges, lost) = log.take();
        assert_eq!(edges.len(), MAX_EDGES);
        assert_eq!(lost, 5);
        assert_eq!(edges[0].at_us, 6);
    }

    #[test]
    fn the_sd_copy_is_handed_over_only_when_the_report_is_out() {
        let mut recorder = Recorder::default();
        recorder.note_dropped("up", timing(0, 0));
        recorder.note_dropped("down", timing(1_000, 1_000));
        assert_eq!(recorder.take_sd_batch(), None);
        let _ = recorder.next_line();
        assert_eq!(recorder.take_sd_batch(), None);
        let _ = recorder.next_line();
        assert_eq!(
            recorder.take_sd_batch().as_deref(),
            Some("itx key=up t=0.0 deb=0.0\nitx key=down t=1.0 deb=0.0\n")
        );
        assert_eq!(recorder.take_sd_batch(), None);
    }

    #[test]
    fn milliseconds_round_to_one_decimal() {
        assert_eq!(ms(0), "0.0");
        assert_eq!(ms(49), "0.0");
        assert_eq!(ms(50), "0.1");
        assert_eq!(ms(1_234_567), "1234.6");
        assert_eq!(ms(-1_250), "-1.3");
        assert_eq!(optional_ms(None), "-");
    }
}
