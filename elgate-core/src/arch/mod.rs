//! Architecture detection and thread management
//!
//! This module is responsible for:
//! - Detecting the CPU topology (cores, NUMA nodes)
//! - Selecting appropriate runtime modes
//! - Building and pinning worker threads

pub mod cpu_info;
pub mod runtime_mode;
pub mod thread_builder;

pub use cpu_info::{CpuInfo, NumaNode};
pub use runtime_mode::RuntimeMode;
pub use thread_builder::{PinningResult, ThreadBuilder, WorkerThread};

/// Get information about the current system's CPU topology
pub fn detect_cpu_topology() -> CpuInfo {
    CpuInfo::detect()
}

/// Select the most appropriate runtime mode for the current system
pub fn select_runtime_mode(cpu_info: &CpuInfo) -> RuntimeMode {
    RuntimeMode::select_for_system(cpu_info)
}

/// Create a new thread builder for worker threads
pub fn create_thread_builder(mode: RuntimeMode, cpu_info: &CpuInfo) -> ThreadBuilder {
    ThreadBuilder::new(mode, cpu_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cpu_topology() {
        let cpu_info = detect_cpu_topology();
        println!("Detected {} logical cores", cpu_info.logical_cores());
        assert!(cpu_info.logical_cores() > 0);
    }

    #[test]
    fn test_select_runtime_mode() {
        let cpu_info = detect_cpu_topology();
        let mode = select_runtime_mode(&cpu_info);
        println!("Selected runtime mode: {:?}", mode);

        // If we have more than one core, we should get PinnedSharded mode
        if cpu_info.logical_cores() > 1 {
            assert!(matches!(mode, RuntimeMode::PinnedSharded { .. }));
        } else {
            assert!(matches!(mode, RuntimeMode::SingleThread));
        }
    }
}
