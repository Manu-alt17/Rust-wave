//! Power-management profiling telemetry for battery investigations.
//!
//! Active only in a diagnostic build that layers
//! `sdkconfig.defaults.pm-profiling` (`CONFIG_PM_PROFILING=y`) over the
//! normal defaults -- see `docs/PHYSICAL_SMOKE_TEST.md`. The normal firmware
//! compiles this down to nothing ([`ENABLED`] is `false`).
//!
//! ESP-IDF's `esp_pm_dump_locks` reports time spent in each power mode and
//! per-lock hold times, cumulative since boot. [`PowerProfileTracker`]
//! turns successive dumps into per-window percentages tagged with what the
//! device was doing, so e.g. "Reader page, panel asleep, idle for a minute"
//! can be read straight off one log line: whether automatic light sleep is
//! actually entered (`light-sleeps`; `min-freq-pct` is only the share of
//! time spent at the 40 MHz floor where light sleep is *permitted*) and which locks hold the
//! chip awake (`pm-profile-lock` lines).

/// Whether this build carries ESP-IDF's PM profiling counters.
pub const ENABLED: bool = cfg!(esp_idf_pm_profiling);

/// Seconds between two profiling log windows.
pub const POWER_PROFILE_LOG_SECONDS: u64 = 60;

/// Cumulative time per power mode (microseconds since boot) plus automatic
/// light-sleep attempt counters, as parsed from one `esp_pm_dump_locks` dump.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PmModeTimes {
    pub light_sleep_us: u64,
    pub apb_min_us: u64,
    pub apb_max_us: u64,
    pub cpu_max_us: u64,
    pub light_sleep_counts: u64,
    pub light_sleep_rejects: u64,
}

impl PmModeTimes {
    fn total_us(self) -> u64 {
        self.light_sleep_us + self.apb_min_us + self.apb_max_us + self.cpu_max_us
    }
}

/// Parse the "Mode stats" and "Sleep stats" sections of an
/// `esp_pm_dump_locks` dump (ESP-IDF v5.5 format). `None` when the dump has
/// no mode table, i.e. the build lacks `CONFIG_PM_PROFILING`.
#[must_use]
pub fn parse_mode_stats(dump: &str) -> Option<PmModeTimes> {
    let mut times = PmModeTimes::default();
    let mut in_modes = false;
    let mut saw_mode = false;
    for line in dump.lines() {
        let line = line.trim();
        if line.starts_with("Mode stats:") {
            in_modes = true;
            continue;
        }
        if line.starts_with("light_sleep_counts:") {
            for field in line.split_whitespace() {
                if let Some(value) = field.strip_prefix("light_sleep_counts:") {
                    times.light_sleep_counts = value.parse().ok()?;
                } else if let Some(value) = field.strip_prefix("light_sleep_reject_counts:") {
                    times.light_sleep_rejects = value.parse().ok()?;
                }
            }
            continue;
        }
        if !in_modes || line.is_empty() || line.starts_with("Mode ") {
            continue;
        }
        if line.starts_with("Sleep stats:") {
            in_modes = false;
            continue;
        }
        // C format `"%-8s  %-3uM%-7s %-10lld  %-2d%%"`: a two-digit frequency
        // or percentage is padded before its unit (`80 M`, `5 %`), which
        // splits it into two tokens. Dropping the lone unit tokens leaves
        // `<mode> <freq> <time_us> <percent>` for every row.
        let mut fields = line
            .split_whitespace()
            .filter(|field| !matches!(*field, "M" | "%"));
        let (Some(mode), Some(_freq), Some(time)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(time_us) = time.parse::<u64>() else {
            continue;
        };
        let slot = match mode {
            "SLEEP" => &mut times.light_sleep_us,
            "APB_MIN" => &mut times.apb_min_us,
            "APB_MAX" => &mut times.apb_max_us,
            "CPU_MAX" => &mut times.cpu_max_us,
            _ => continue,
        };
        *slot = time_us;
        saw_mode = true;
    }
    saw_mode.then_some(times)
}

/// Share of one profiling window spent in each power mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmWindowSummary {
    pub window_ms: u64,
    /// Share at ESP-IDF's lowest DFS mode (named `SLEEP` in its dump): CPU
    /// at the frequency floor with light sleep permitted. Actual light-sleep
    /// entries are counted separately in [`Self::light_sleeps`].
    pub min_freq_pct: u64,
    pub apb_min_pct: u64,
    pub apb_max_pct: u64,
    pub cpu_max_pct: u64,
    pub light_sleeps: u64,
    pub light_sleep_rejects: u64,
}

/// Turns cumulative-since-boot dumps into per-window deltas.
#[derive(Debug, Default)]
pub struct PowerProfileTracker {
    previous: Option<PmModeTimes>,
}

impl PowerProfileTracker {
    /// Record one dump. Returns the window since the previous one, or
    /// `None` for the first observation (nothing to compare against yet).
    pub fn observe(&mut self, current: PmModeTimes) -> Option<PmWindowSummary> {
        let previous = self.previous.replace(current)?;
        let delta = PmModeTimes {
            light_sleep_us: current
                .light_sleep_us
                .saturating_sub(previous.light_sleep_us),
            apb_min_us: current.apb_min_us.saturating_sub(previous.apb_min_us),
            apb_max_us: current.apb_max_us.saturating_sub(previous.apb_max_us),
            cpu_max_us: current.cpu_max_us.saturating_sub(previous.cpu_max_us),
            light_sleep_counts: current
                .light_sleep_counts
                .saturating_sub(previous.light_sleep_counts),
            light_sleep_rejects: current
                .light_sleep_rejects
                .saturating_sub(previous.light_sleep_rejects),
        };
        let total = delta.total_us().max(1);
        let pct = |value: u64| value * 100 / total;
        Some(PmWindowSummary {
            window_ms: delta.total_us() / 1000,
            min_freq_pct: pct(delta.light_sleep_us),
            apb_min_pct: pct(delta.apb_min_us),
            apb_max_pct: pct(delta.apb_max_us),
            cpu_max_pct: pct(delta.cpu_max_us),
            light_sleeps: delta.light_sleep_counts,
            light_sleep_rejects: delta.light_sleep_rejects,
        })
    }

    /// Capture a dump and log this window's summary plus the per-lock table,
    /// tagged with `context` (route, panel and radio state). No-op unless
    /// [`ENABLED`].
    pub fn log_window(&mut self, context: &str) {
        if !ENABLED {
            return;
        }
        let dump = match capture_pm_dump() {
            Ok(dump) => dump,
            Err(error) => {
                log::warn!("rustmix-wave=pm-profile status=capture-failed error={error}");
                return;
            }
        };
        let Some(times) = parse_mode_stats(&dump) else {
            log::warn!("rustmix-wave=pm-profile status=unparsed-dump");
            return;
        };
        let mut lines = vec![match self.observe(times) {
            Some(window) => format!(
                "rustmix-wave=pm-profile window-ms={} min-freq-pct={} apb-min-pct={} apb-max-pct={} cpu-max-pct={} light-sleeps={} light-sleep-rejects={} {context}",
                window.window_ms,
                window.min_freq_pct,
                window.apb_min_pct,
                window.apb_max_pct,
                window.cpu_max_pct,
                window.light_sleeps,
                window.light_sleep_rejects,
            ),
            None => format!("rustmix-wave=pm-profile status=baseline {context}"),
        }];
        // Per-lock rows are cumulative since boot: which locks keep the chip
        // out of light sleep (NO_LIGHT_SLEEP) or at a raised frequency.
        let mut in_locks = false;
        for line in dump.lines() {
            if line.starts_with("Lock stats:") {
                in_locks = true;
                continue;
            }
            if line.trim().is_empty() || line.starts_with("Mode stats:") {
                in_locks = false;
            }
            if in_locks && !line.starts_with("Name ") {
                lines.push(format!("rustmix-wave=pm-profile-lock {}", line.trim_end()));
            }
        }
        for line in &lines {
            log::info!("{line}");
        }
        append_to_sd_log(&lines);
    }
}

/// SD copy of every profiling window. Automatic light sleep is blocked while
/// a USB host is attached (`CONFIG_USJ_NO_AUTO_LS_ON_CONNECTION`: the USB
/// Serial/JTAG console cannot survive it), so the windows that matter are
/// recorded on battery or a wall charger with no serial monitor attached,
/// and read back from the card afterwards.
pub const POWER_PROFILE_SD_LOG_PATH: &str = "/sdcard/RUSTMIX/PMPROF.TXT";

/// Append one window's lines to [`POWER_PROFILE_SD_LOG_PATH`], prefixed with
/// uptime so windows from different boots can be told apart. Best effort:
/// a missing card only costs the SD copy, never the serial log.
fn append_to_sd_log(lines: &[String]) {
    use std::io::Write;
    let uptime_ms = uptime_ms();
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(POWER_PROFILE_SD_LOG_PATH)
        .and_then(|mut file| {
            for line in lines {
                writeln!(file, "uptime-ms={uptime_ms} {line}")?;
            }
            Ok(())
        });
    if let Err(error) = result {
        log::warn!(
            "rustmix-wave=pm-profile status=sd-log-failed path={POWER_PROFILE_SD_LOG_PATH} error={error}"
        );
    }
}

#[cfg(target_os = "espidf")]
fn uptime_ms() -> i64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() / 1000 }
}

#[cfg(not(target_os = "espidf"))]
fn uptime_ms() -> i64 {
    0
}

/// Announce at boot whether this is the PM-profiling diagnostic build, so a
/// capture session can confirm the right firmware was flashed before
/// waiting for the first window.
pub fn log_build_status() {
    if ENABLED {
        log::info!(
            "rustmix-wave=pm-profile status=enabled window-seconds={POWER_PROFILE_LOG_SECONDS}"
        );
    }
}

/// Capture `esp_pm_dump_locks` output into a string via `open_memstream`.
#[cfg(all(target_os = "espidf", esp_idf_pm_profiling))]
fn capture_pm_dump() -> Result<String, &'static str> {
    use esp_idf_svc::sys;
    let mut buffer: *mut core::ffi::c_char = core::ptr::null_mut();
    let mut size: usize = 0;
    let stream = unsafe { sys::open_memstream(&mut buffer, &mut size) };
    if stream.is_null() {
        return Err("open-memstream");
    }
    let status = unsafe { sys::esp_pm_dump_locks(stream) };
    // Closing finalizes `buffer`/`size`.
    unsafe { sys::fclose(stream) };
    if buffer.is_null() {
        return Err("empty-buffer");
    }
    let text =
        String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), size) })
            .into_owned();
    unsafe { sys::free(buffer.cast()) };
    if status == sys::ESP_OK {
        Ok(text)
    } else {
        Err("esp-pm-dump-locks")
    }
}

#[cfg(not(all(target_os = "espidf", esp_idf_pm_profiling)))]
fn capture_pm_dump() -> Result<String, &'static str> {
    Err("pm-profiling-not-built")
}

#[cfg(test)]
mod tests {
    use super::{parse_mode_stats, PmModeTimes, PowerProfileTracker};

    /// Shape of a real ESP-IDF v5.5 `esp_pm_dump_locks` dump.
    fn sample_dump(sleep: u64, apb_min: u64, apb_max: u64, cpu_max: u64, counts: u64) -> String {
        format!(
            "Time since bootup: {} us\n\
             Lock stats:\n\
             Name            Type            Arg    Active    Total_count    Time(us)        Time(%)\n\
             rtos0           CPU_FREQ_MAX    0      1         5321           812345          9  %\n\
             wifi            APB_FREQ_MAX    0      0         120            22000           1  %\n\
             \n\
             Mode stats:\n\
             Mode      CPU_freq    Time(us)    Time(%)\n\
             {}{}{}{}\
             \n\
             Sleep stats:\n\
             light_sleep_counts:{counts}  light_sleep_reject_counts:3\n",
            sleep + apb_min + apb_max + cpu_max,
            mode_row("SLEEP", 40, sleep, 10),
            mode_row("APB_MIN", 40, apb_min, 60),
            mode_row("APB_MAX", 80, apb_max, 5),
            mode_row("CPU_MAX", 240, cpu_max, 25),
        )
    }

    /// One "Mode stats" row exactly as ESP-IDF v5.5 prints it,
    /// `"%-8s  %-3uM%-7s %-10lld  %-2d%%\n"` (`esp_pm_impl_dump_stats`):
    /// two-digit frequencies and one-digit percentages come out as
    /// `40 M` / `5 %`, the shape the first on-device parser misread.
    fn mode_row(mode: &str, freq_mhz: u32, time_us: u64, percent: u32) -> String {
        format!(
            "{mode:<8}  {freq_mhz:<3}M{:<7} {time_us:<10}  {percent:<2}%\n",
            ""
        )
    }

    #[test]
    fn parses_rows_exactly_as_logged_on_the_device() {
        // Verbatim shape from a field log (window of ~60 s, mostly APB_MAX).
        let dump = "Mode stats:\n\
                    Mode      CPU_freq    Time(us)    Time(%)\n\
                    SLEEP     40 M        0           0 %\n\
                    APB_MIN   40 M        0           0 %\n\
                    APB_MAX   80 M        52000000    87%\n\
                    CPU_MAX   240M        7580000     12%\n";
        let times = parse_mode_stats(dump).unwrap();
        assert_eq!(times.apb_max_us, 52_000_000);
        assert_eq!(times.cpu_max_us, 7_580_000);
        assert_eq!(times.light_sleep_us, 0);
    }

    #[test]
    fn parses_mode_and_sleep_stats() {
        let times = parse_mode_stats(&sample_dump(100, 600, 100, 200, 7)).unwrap();
        assert_eq!(
            times,
            PmModeTimes {
                light_sleep_us: 100,
                apb_min_us: 600,
                apb_max_us: 100,
                cpu_max_us: 200,
                light_sleep_counts: 7,
                light_sleep_rejects: 3,
            }
        );
    }

    #[test]
    fn a_dump_without_profiling_counters_is_not_parsed() {
        let dump = "Lock stats:\nName            Type            Arg    Active\nrtos0           CPU_FREQ_MAX    0      1\n";
        assert_eq!(parse_mode_stats(dump), None);
    }

    #[test]
    fn a_dump_without_light_sleep_enabled_reports_zero_sleep() {
        let dump = sample_dump(0, 600, 100, 200, 0)
            .lines()
            .filter(|line| !line.trim_start().starts_with("SLEEP"))
            .filter(|line| !line.contains("light_sleep_counts"))
            .collect::<Vec<_>>()
            .join("\n");
        let times = parse_mode_stats(&dump).unwrap();
        assert_eq!(times.light_sleep_us, 0);
        assert_eq!(times.apb_min_us, 600);
    }

    #[test]
    fn windows_report_deltas_between_dumps() {
        let mut tracker = PowerProfileTracker::default();
        let first = parse_mode_stats(&sample_dump(1_000, 5_000, 1_000, 3_000, 2)).unwrap();
        assert!(tracker.observe(first).is_none());
        // Next window: 60 s, of which 45 s light sleep, 12 s APB_MIN, 3 s CPU_MAX.
        let second = parse_mode_stats(&sample_dump(
            1_000 + 45_000_000,
            5_000 + 12_000_000,
            1_000,
            3_000 + 3_000_000,
            2 + 900,
        ))
        .unwrap();
        let window = tracker.observe(second).unwrap();
        assert_eq!(window.window_ms, 60_000);
        assert_eq!(window.min_freq_pct, 75);
        assert_eq!(window.apb_min_pct, 20);
        assert_eq!(window.apb_max_pct, 0);
        assert_eq!(window.cpu_max_pct, 5);
        assert_eq!(window.light_sleeps, 900);
        assert_eq!(window.light_sleep_rejects, 0);
    }
}
