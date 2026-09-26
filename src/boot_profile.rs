//! Boot timing profiler (temporary diagnostic).
//!
//! Records named spans and point marks from power-up until the first main
//! loop settles, so every boot phase can be timed on real hardware without a
//! serial monitor attached. Nothing is logged while recording -- each UART
//! line costs ~87 us per character at 115200 baud and would distort the very
//! timings being measured -- so entries stay in RAM (a mutex-guarded `Vec`
//! push, a few microseconds each) until [`flush_to_file`] appends them to SD
//! and [`finish`] prints the whole report once boot is over.
//!
//! Timestamps come from `esp_timer_get_time`, so the first entry's start
//! also shows how long ROM, bootloader and ESP-IDF startup took before
//! `firmware::run` was entered. On the host they are relative to the first
//! call, which keeps the module usable from unit-tested code.

use std::{
    cell::Cell,
    fmt::Write as _,
    io::Write as _,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

/// Upper bound on recorded entries, so a profiler left active by mistake
/// can't grow without limit.
const MAX_ENTRIES: usize = 512;

static ACTIVE: AtomicBool = AtomicBool::new(true);
static STATE: Mutex<ProfileState> = Mutex::new(ProfileState {
    entries: Vec::new(),
    written: 0,
    dropped: 0,
});

thread_local! {
    static DEPTH: Cell<u8> = const { Cell::new(0) };
}

struct ProfileState {
    entries: Vec<Entry>,
    /// Entries before this index are already on SD.
    written: usize,
    dropped: usize,
}

struct Entry {
    name: &'static str,
    detail: Option<String>,
    start_us: i64,
    /// `None` for a point mark.
    end_us: Option<i64>,
    depth: u8,
}

/// Microseconds on the profiler's clock.
#[must_use]
pub fn now_us() -> i64 {
    #[cfg(target_os = "espidf")]
    {
        unsafe { esp_idf_svc::sys::esp_timer_get_time() }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        use std::{sync::OnceLock, time::Instant};
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(Instant::now);
        i64::try_from(origin.elapsed().as_micros()).unwrap_or(i64::MAX)
    }
}

#[must_use]
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn push(entry: Entry) {
    let Ok(mut state) = STATE.lock() else {
        return;
    };
    if state.entries.len() >= MAX_ENTRIES {
        state.dropped += 1;
        return;
    }
    state.entries.push(entry);
}

/// Record a zero-duration event.
pub fn mark(name: &'static str) {
    mark_with(name, None::<&str>);
}

/// Record a zero-duration event with a free-form detail.
pub fn mark_with(name: &'static str, detail: Option<impl std::fmt::Display>) {
    if !is_active() {
        return;
    }
    push(Entry {
        name,
        detail: detail.map(|detail| detail.to_string()),
        start_us: now_us(),
        end_us: None,
        depth: DEPTH.with(Cell::get),
    });
}

/// Start a span that is recorded when the returned guard drops.
#[must_use = "the span ends when this guard is dropped"]
pub fn span(name: &'static str) -> Span {
    if !is_active() {
        return Span { inner: None };
    }
    let depth = DEPTH.with(|depth| {
        let current = depth.get();
        depth.set(current.saturating_add(1));
        current
    });
    Span {
        inner: Some(SpanInner {
            name,
            detail: None,
            start_us: now_us(),
            depth,
        }),
    }
}

pub struct Span {
    inner: Option<SpanInner>,
}

struct SpanInner {
    name: &'static str,
    detail: Option<String>,
    start_us: i64,
    depth: u8,
}

impl Span {
    /// Attach a detail (file name, outcome, stage...) to this span.
    pub fn detail(&mut self, detail: impl std::fmt::Display) {
        if let Some(inner) = self.inner.as_mut() {
            inner.detail = Some(detail.to_string());
        }
    }

    /// End the span now instead of at the end of the enclosing scope.
    pub fn end(self) {}
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        DEPTH.with(|depth| depth.set(inner.depth));
        push(Entry {
            name: inner.name,
            detail: inner.detail,
            start_us: inner.start_us,
            end_us: Some(now_us()),
            depth: inner.depth,
        });
    }
}

fn format_entry(entry: &Entry, out: &mut String) {
    let _ = write!(
        out,
        "boot-profile t-ms={:.1} ",
        entry.start_us as f64 / 1000.0
    );
    match entry.end_us {
        Some(end_us) => {
            let _ = write!(
                out,
                "dur-ms={:.1} ",
                (end_us - entry.start_us) as f64 / 1000.0
            );
        }
        None => out.push_str("mark "),
    }
    let _ = write!(
        out,
        "depth={} {}{}",
        entry.depth,
        "  ".repeat(entry.depth.into()),
        entry.name
    );
    if let Some(detail) = entry.detail.as_deref() {
        let _ = write!(out, " [{detail}]");
    }
}

/// Entries sorted by start time (spans are pushed when they end, so nested
/// spans would otherwise precede their parent).
fn sorted_lines(entries: &[Entry]) -> Vec<String> {
    let mut order: Vec<&Entry> = entries.iter().collect();
    order.sort_by_key(|entry| (entry.start_us, entry.depth));
    order
        .into_iter()
        .map(|entry| {
            let mut line = String::new();
            format_entry(entry, &mut line);
            line
        })
        .collect()
}

/// Append every entry not yet written to `path`, under `header`. Best-effort:
/// returns the error instead of panicking or retrying.
pub fn flush_to_file(path: &str, header: &str) -> std::io::Result<()> {
    let _span = span("boot-profile-flush");
    let lines = {
        let Ok(mut state) = STATE.lock() else {
            return Ok(());
        };
        let start = state.written;
        state.written = state.entries.len();
        let mut lines = sorted_lines(&state.entries[start..]);
        if state.dropped > 0 {
            lines.push(format!("boot-profile dropped-entries={}", state.dropped));
        }
        lines
    };
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut body = String::with_capacity(lines.len() * 80 + header.len() + 1);
    body.push_str(header);
    body.push('\n');
    for line in &lines {
        body.push_str(line);
        body.push('\n');
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(body.as_bytes())
}

/// Stop recording and return every entry recorded since boot, sorted, for
/// the caller to log. Entries already flushed to SD are included again, so
/// the serial report is complete on its own.
#[must_use]
pub fn finish() -> Vec<String> {
    ACTIVE.store(false, Ordering::Relaxed);
    let Ok(mut state) = STATE.lock() else {
        return Vec::new();
    };
    let lines = sorted_lines(&state.entries);
    state.entries = Vec::new();
    state.written = 0;
    lines
}

#[cfg(test)]
mod tests {
    use super::{mark, sorted_lines, span, Entry};

    #[test]
    fn nested_spans_sort_parent_first() {
        let entries = vec![
            Entry {
                name: "child",
                detail: None,
                start_us: 20,
                end_us: Some(30),
                depth: 1,
            },
            Entry {
                name: "parent",
                detail: None,
                start_us: 10,
                end_us: Some(40),
                depth: 0,
            },
            Entry {
                name: "point",
                detail: Some("x".into()),
                start_us: 35,
                end_us: None,
                depth: 1,
            },
        ];
        let lines = sorted_lines(&entries);
        assert!(lines[0].contains("parent") && lines[0].contains("dur-ms=0.0"));
        assert!(lines[1].contains("child") && lines[1].contains("depth=1"));
        assert!(lines[2].contains("mark") && lines[2].contains("point [x]"));
    }

    #[test]
    fn guards_record_without_panicking() {
        let mut outer = span("outer");
        outer.detail("ok");
        {
            let _inner = span("inner");
            mark("point");
        }
        outer.end();
    }
}
