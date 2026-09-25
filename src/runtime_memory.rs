//! Runtime stack and heap telemetry for worker-boundary hardening.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RuntimeMemorySnapshot {
    pub main_stack_high_water_bytes: usize,
    pub heap_free_internal_bytes: usize,
    pub heap_largest_internal_block_bytes: usize,
    pub heap_free_psram_bytes: usize,
    /// Number of distinct free blocks in the internal-RAM heap. High alongside
    /// a small `heap_largest_internal_block_bytes` (relative to
    /// `heap_free_internal_bytes`) is the fingerprint of fragmentation, not
    /// genuine low memory.
    pub heap_internal_free_blocks: usize,
    /// Lowest `heap_free_internal_bytes` has ever been since boot. Tracks
    /// worst-case pressure even between two snapshots that both look fine.
    pub heap_internal_minimum_free_bytes: usize,
    /// Largest free block restricted to DMA-capable memory. A worker stack
    /// itself has no DMA requirement, but this tells whether whatever is
    /// fragmenting internal RAM (Wi-Fi's own buffers are DMA-capable) is
    /// also capping this ceiling -- relevant because DMA-capable memory can
    /// never be satisfied from PSRAM, so if this number is the same as
    /// `heap_largest_internal_block_bytes`, moving more allocations to
    /// PSRAM only helps as much as those allocations weren't DMA-bound.
    pub heap_largest_dma_block_bytes: usize,
}

#[cfg(target_os = "espidf")]
impl RuntimeMemorySnapshot {
    #[must_use]
    pub fn capture() -> Self {
        use esp_idf_svc::sys;
        let mut internal_info = unsafe { core::mem::zeroed::<sys::multi_heap_info_t>() };
        unsafe {
            sys::heap_caps_get_info(&mut internal_info, sys::MALLOC_CAP_INTERNAL as u32);
        }
        Self {
            // ESP-IDF's FreeRTOS port reports the minimum remaining stack
            // margin for the current task in bytes.
            main_stack_high_water_bytes: unsafe {
                sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) as usize
            },
            heap_free_internal_bytes: internal_info.total_free_bytes,
            heap_largest_internal_block_bytes: internal_info.largest_free_block,
            heap_free_psram_bytes: unsafe {
                sys::heap_caps_get_free_size(sys::MALLOC_CAP_SPIRAM as u32)
            },
            heap_internal_free_blocks: internal_info.free_blocks,
            heap_internal_minimum_free_bytes: internal_info.minimum_free_bytes,
            heap_largest_dma_block_bytes: unsafe {
                sys::heap_caps_get_largest_free_block(sys::MALLOC_CAP_DMA as u32)
            },
        }
    }
}

#[cfg(not(target_os = "espidf"))]
impl RuntimeMemorySnapshot {
    #[must_use]
    pub fn capture() -> Self {
        Self::default()
    }
}

pub fn log_runtime_memory(boundary: &str) {
    let snapshot = RuntimeMemorySnapshot::capture();
    log::info!(
        "rustmix-wave=runtime-memory boundary={} main-stack-high-water-bytes={} heap-free-internal-bytes={} heap-largest-internal-block-bytes={} heap-free-psram-bytes={} heap-internal-free-blocks={} heap-internal-minimum-free-bytes={} heap-largest-dma-block-bytes={}",
        sanitize_marker(boundary),
        snapshot.main_stack_high_water_bytes,
        snapshot.heap_free_internal_bytes,
        snapshot.heap_largest_internal_block_bytes,
        snapshot.heap_free_psram_bytes,
        snapshot.heap_internal_free_blocks,
        snapshot.heap_internal_minimum_free_bytes,
        snapshot.heap_largest_dma_block_bytes
    );
}

fn sanitize_marker(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::RuntimeMemorySnapshot;

    #[test]
    fn host_snapshot_is_safe_without_espidf_heap_apis() {
        assert_eq!(
            RuntimeMemorySnapshot::capture(),
            RuntimeMemorySnapshot::default()
        );
    }
}
