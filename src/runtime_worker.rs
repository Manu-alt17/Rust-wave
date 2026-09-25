//! Reusable short-lived worker boundary.
//!
//! Heavy operations receive named stack budgets and return compact heap-owned
//! results to the main hardware-orchestration loop. Panel SPI ownership stays
//! on the main task.

use core::fmt::{self, Display};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Debug)]
pub enum NamedWorkerError<E> {
    Start(std::io::Error),
    Panicked,
    Operation(E),
}

impl<E: Display> Display for NamedWorkerError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(error) => write!(formatter, "worker start failed: {error}"),
            Self::Panicked => formatter.write_str("worker panicked"),
            Self::Operation(error) => Display::fmt(error, formatter),
        }
    }
}

impl<E: Display + fmt::Debug> std::error::Error for NamedWorkerError<E> {}

pub fn run_named_worker<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<T, NamedWorkerError<E>>
where
    T: Send + 'static,
    E: Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    log::info!(
        "rustmix-wave=worker-boundary name={name} status=starting stack-bytes={stack_bytes}"
    );
    crate::runtime_memory::log_runtime_memory(&format!("before-worker-{name}"));
    let worker = std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack_bytes)
        .spawn(task)
        .map_err(|error| {
            log::warn!(
                "rustmix-wave=worker-boundary name={name} status=start-failed error={error}"
            );
            NamedWorkerError::Start(error)
        })?;
    let result = worker.join().map_err(|_| {
        log::warn!("rustmix-wave=worker-boundary name={name} status=panicked");
        NamedWorkerError::Panicked
    })?;
    crate::runtime_memory::log_runtime_memory(&format!("after-worker-{name}"));
    match result {
        Ok(value) => {
            log::info!("rustmix-wave=worker-boundary name={name} status=completed");
            Ok(value)
        }
        Err(error) => {
            log::warn!("rustmix-wave=worker-boundary name={name} status=failed error={error}");
            Err(NamedWorkerError::Operation(error))
        }
    }
}

/// Non-blocking counterpart to [`run_named_worker`]: spawns the same kind of
/// short-lived named-stack worker, but returns immediately with a
/// [`Receiver`] instead of joining the thread. Poll it with
/// [`poll_named_worker`] from a loop that must keep running (e.g. the main
/// hardware-orchestration loop, which must keep draining input and
/// redrawing) while the operation is in flight -- unlike `run_named_worker`,
/// whose caller is blocked for the operation's whole duration, this is safe
/// to call from that loop directly.
pub fn spawn_named_worker<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<Receiver<Result<T, E>>, std::io::Error>
where
    T: Send + 'static,
    E: Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    log::info!(
        "rustmix-wave=worker-boundary name={name} status=starting stack-bytes={stack_bytes}"
    );
    crate::runtime_memory::log_runtime_memory(&format!("before-worker-{name}"));
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(stack_bytes)
        .spawn(move || {
            // Nothing reads this send's `Result` when the receiver has
            // already been dropped (e.g. the caller gave up on a stale
            // in-flight request across a sleep/resume boundary) -- that is
            // an expected, harmless outcome, not an error to report.
            let _ = sender.send(task());
        })
        .map_err(|error| {
            log::warn!(
                "rustmix-wave=worker-boundary name={name} status=start-failed error={error}"
            );
            error
        })?;
    Ok(receiver)
}

/// Non-blocking poll for a worker started with [`spawn_named_worker`].
/// Returns `None` while the operation is still running -- call again on a
/// later loop iteration. A disconnected channel (the worker thread panicked
/// before sending) is reported as [`NamedWorkerError::Panicked`], matching
/// `run_named_worker`'s behavior for a joined panic.
pub fn poll_named_worker<T, E: Display>(
    name: &'static str,
    receiver: &Receiver<Result<T, E>>,
) -> Option<Result<T, NamedWorkerError<E>>> {
    match receiver.try_recv() {
        Ok(result) => {
            crate::runtime_memory::log_runtime_memory(&format!("after-worker-{name}"));
            match result {
                Ok(value) => {
                    log::info!("rustmix-wave=worker-boundary name={name} status=completed");
                    Some(Ok(value))
                }
                Err(error) => {
                    log::warn!(
                        "rustmix-wave=worker-boundary name={name} status=failed error={error}"
                    );
                    Some(Err(NamedWorkerError::Operation(error)))
                }
            }
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            crate::runtime_memory::log_runtime_memory(&format!("after-worker-{name}"));
            log::warn!("rustmix-wave=worker-boundary name={name} status=panicked");
            Some(Err(NamedWorkerError::Panicked))
        }
    }
}

/// Like [`run_named_worker`], but asks ESP-IDF to allocate the spawned
/// thread's stack from PSRAM instead of the much smaller internal-SRAM
/// pool. Internal SRAM is shared by Wi-Fi/lwIP buffers and every other
/// worker stack in the app; on this hardware (8 MB octal PSRAM, ~160 KB
/// internal SRAM) merely being Wi-Fi-connected has been observed to leave
/// the largest contiguous internal block *at or below* one 64 KB worker
/// stack's size, with essentially none of the 8 MB of PSRAM ever touched —
/// see the runtime-memory log lines this module already emits.
///
/// # When this is safe to use
/// A PSRAM-backed stack is unusable for the short window ESP-IDF disables
/// the flash cache during a raw flash write/erase (NVS, OTA): PSRAM, like
/// flash, is only reachable through that same cache-mapped bus. ESP-IDF's
/// own flash operations already pause every other core for that window, so
/// a PSRAM-stack thread merely stalls rather than corrupting anything — but
/// only reuse this for workers that read the SD card and compute in RAM,
/// never touch flash directly, and never run from an ISR. `open_epub_on_worker`
/// and `CoverCache::generate_thumbnail` both satisfy this; nothing OTA- or
/// flash-adjacent should call this helper.
///
/// # Scope
/// `esp_pthread_set_cfg` is thread-*local* to the calling task and applies
/// only to that task's next `pthread_create` (`inherit_cfg` is left
/// `false`, so the spawned worker's own descendants, if any, are
/// unaffected). This restores the calling task's default configuration
/// immediately after spawning, so nothing else spawned later from that same
/// calling task is affected either.
#[cfg(target_os = "espidf")]
pub fn run_named_worker_in_psram<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<T, NamedWorkerError<E>>
where
    T: Send + 'static,
    E: Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    use esp_idf_svc::sys::{esp_pthread_get_default_config, esp_pthread_set_cfg};

    let mut cfg = unsafe { esp_pthread_get_default_config() };
    cfg.stack_alloc_caps = esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    let status = unsafe { esp_pthread_set_cfg(&cfg) };
    if status != 0 {
        log::warn!(
            "rustmix-wave=worker-boundary name={name} status=psram-cfg-failed error-code={status}"
        );
    }

    let result = run_named_worker(name, stack_bytes, task);

    let restore = unsafe { esp_pthread_get_default_config() };
    let restore_status = unsafe { esp_pthread_set_cfg(&restore) };
    if restore_status != 0 {
        log::warn!(
            "rustmix-wave=worker-boundary name={name} status=psram-cfg-restore-failed error-code={restore_status}"
        );
    }

    result
}

/// Host/test builds have no PSRAM concept: fall back to the normal
/// internal-stack worker so callers stay portable.
#[cfg(not(target_os = "espidf"))]
pub fn run_named_worker_in_psram<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<T, NamedWorkerError<E>>
where
    T: Send + 'static,
    E: Display + Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    run_named_worker(name, stack_bytes, task)
}

/// Non-blocking counterpart to [`run_named_worker_in_psram`]: same
/// PSRAM-backed stack (needed once Wi-Fi is connected -- see that function's
/// doc comment for the field-confirmed internal-SRAM fragmentation this
/// avoids), but returns immediately with a [`Receiver`] instead of joining
/// the thread. Poll it with [`poll_named_worker`] from a loop that must keep
/// running while the operation is in flight.
#[cfg(target_os = "espidf")]
pub fn spawn_named_worker_in_psram<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<Receiver<Result<T, E>>, std::io::Error>
where
    T: Send + 'static,
    E: Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    use esp_idf_svc::sys::{esp_pthread_get_default_config, esp_pthread_set_cfg};

    let mut cfg = unsafe { esp_pthread_get_default_config() };
    cfg.stack_alloc_caps = esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    let status = unsafe { esp_pthread_set_cfg(&cfg) };
    if status != 0 {
        log::warn!(
            "rustmix-wave=worker-boundary name={name} status=psram-cfg-failed error-code={status}"
        );
    }

    let result = spawn_named_worker(name, stack_bytes, task);

    let restore = unsafe { esp_pthread_get_default_config() };
    let restore_status = unsafe { esp_pthread_set_cfg(&restore) };
    if restore_status != 0 {
        log::warn!(
            "rustmix-wave=worker-boundary name={name} status=psram-cfg-restore-failed error-code={restore_status}"
        );
    }

    result
}

/// Host/test builds have no PSRAM concept: fall back to the normal
/// internal-stack worker so callers stay portable.
#[cfg(not(target_os = "espidf"))]
pub fn spawn_named_worker_in_psram<T, E, F>(
    name: &'static str,
    stack_bytes: usize,
    task: F,
) -> Result<Receiver<Result<T, E>>, std::io::Error>
where
    T: Send + 'static,
    E: Send + 'static,
    F: FnOnce() -> Result<T, E> + Send + 'static,
{
    spawn_named_worker(name, stack_bytes, task)
}

#[cfg(test)]
mod tests {
    use super::{
        poll_named_worker, run_named_worker, run_named_worker_in_psram, spawn_named_worker,
        spawn_named_worker_in_psram, NamedWorkerError,
    };

    #[test]
    fn returns_compact_result_from_named_short_lived_worker() {
        let result = run_named_worker("unit-worker", 16 * 1024, || Ok::<_, String>(42)).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn psram_worker_falls_back_to_the_normal_worker_off_device() {
        let result =
            run_named_worker_in_psram("unit-worker-psram", 16 * 1024, || Ok::<_, String>(7))
                .unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn spawned_psram_worker_falls_back_to_the_normal_spawn_off_device() {
        let receiver =
            spawn_named_worker_in_psram("unit-worker-spawn-psram", 16 * 1024, || {
                Ok::<_, String>(13)
            })
            .unwrap();
        let value = loop {
            if let Some(result) = poll_named_worker("unit-worker-spawn-psram", &receiver) {
                break result.unwrap();
            }
        };
        assert_eq!(value, 13);
    }

    #[test]
    fn spawned_worker_reports_pending_then_completed_without_blocking_caller() {
        let receiver =
            spawn_named_worker("unit-worker-spawn", 16 * 1024, || Ok::<_, String>(99)).unwrap();
        let value = loop {
            if let Some(result) = poll_named_worker("unit-worker-spawn", &receiver) {
                break result.unwrap();
            }
        };
        assert_eq!(value, 99);
    }

    #[test]
    fn spawned_worker_reports_a_panic_as_a_disconnected_channel() {
        let receiver = spawn_named_worker("unit-worker-panic", 16 * 1024, || -> Result<(), String> {
            panic!("deliberate unit-test panic")
        })
        .unwrap();
        let error = loop {
            if let Some(result) = poll_named_worker("unit-worker-panic", &receiver) {
                break result.unwrap_err();
            }
        };
        assert!(matches!(error, NamedWorkerError::Panicked));
    }
}
