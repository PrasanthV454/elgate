//! Runtime modes for Elgate
//!
//! This module defines the different operational modes that Elgate can run in,
//! based on hardware capabilities and configuration.

use crate::arch::cpu_info::CpuInfo;
use std::fmt;

/// Represents the different runtime modes that Elgate can operate in
#[derive(Clone, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Single-threaded mode for environments with limited resources
    /// or when maximum simplicity is desired
    SingleThread,

    /// Multi-threaded mode with threads pinned to specific cores
    /// and NUMA-aware work distribution
    PinnedSharded {
        /// Number of worker threads to create
        worker_count: usize,
        /// Whether to maintain NUMA locality
        numa_aware: bool,
    },

    /// Force single-threaded mode regardless of available resources
    /// (useful for debugging and testing)
    ForcedSingle,

    /// A test stub that doesn't create real threads
    /// (useful for unit testing)
    TestStub,
}

impl fmt::Debug for RuntimeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SingleThread => write!(f, "SingleThread"),
            Self::PinnedSharded {
                worker_count,
                numa_aware,
            } => {
                write!(
                    f,
                    "PinnedSharded(workers={}, numa={})",
                    worker_count, numa_aware
                )
            }
            Self::ForcedSingle => write!(f, "ForcedSingle"),
            Self::TestStub => write!(f, "TestStub"),
        }
    }
}

impl RuntimeMode {
    /// Selects the most appropriate runtime mode based on system capabilities
    pub fn select_for_system(cpu_info: &CpuInfo) -> Self {
        let logical_cores = cpu_info.logical_cores();

        // For very small systems, use single-threaded mode
        if logical_cores <= 1 {
            return Self::SingleThread;
        }

        // For multi-core systems, use pinned sharded mode
        let worker_count = if logical_cores <= 4 {
            // For 2-4 cores, use all cores
            logical_cores
        } else {
            // For larger systems, leave one core for the OS
            logical_cores - 1
        };

        Self::PinnedSharded {
            worker_count,
            numa_aware: cpu_info.is_numa_available(),
        }
    }

    /// Returns the number of worker threads for this mode
    pub fn worker_count(&self) -> usize {
        match self {
            Self::SingleThread | Self::ForcedSingle | Self::TestStub => 1,
            Self::PinnedSharded { worker_count, .. } => *worker_count,
        }
    }

    /// Checks if this mode uses NUMA awareness
    pub fn is_numa_aware(&self) -> bool {
        match self {
            Self::PinnedSharded { numa_aware, .. } => *numa_aware,
            _ => false,
        }
    }

    /// Checks if this mode supports thread pinning
    pub fn supports_pinning(&self) -> bool {
        match self {
            Self::PinnedSharded { .. } => true,
            _ => false,
        }
    }

    /// Returns a human-readable description of this mode
    pub fn description(&self) -> String {
        match self {
            Self::SingleThread => "Single-threaded mode".to_string(),
            Self::PinnedSharded {
                worker_count,
                numa_aware,
            } => {
                format!(
                    "Pinned sharded mode with {} workers (NUMA-aware: {})",
                    worker_count,
                    if *numa_aware { "yes" } else { "no" }
                )
            }
            Self::ForcedSingle => "Forced single-threaded mode".to_string(),
            Self::TestStub => "Test stub mode".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_runtime_mode() {
        // Test single-core system
        let cpu_info = CpuInfo::mock(1, 0);
        let mode = RuntimeMode::select_for_system(&cpu_info);
        assert!(matches!(mode, RuntimeMode::SingleThread));

        // Test dual-core system
        let cpu_info = CpuInfo::mock(2, 0);
        let mode = RuntimeMode::select_for_system(&cpu_info);
        assert!(matches!(
            mode,
            RuntimeMode::PinnedSharded {
                worker_count: 2,
                ..
            }
        ));

        // Test 8-core system
        let cpu_info = CpuInfo::mock(8, 0);
        let mode = RuntimeMode::select_for_system(&cpu_info);
        assert!(matches!(
            mode,
            RuntimeMode::PinnedSharded {
                worker_count: 7,
                ..
            }
        ));

        // Test 8-core system with 2 NUMA nodes
        let cpu_info = CpuInfo::mock(8, 2);
        let mode = RuntimeMode::select_for_system(&cpu_info);
        if let RuntimeMode::PinnedSharded {
            worker_count,
            numa_aware,
        } = mode
        {
            assert_eq!(worker_count, 7);
            assert!(numa_aware);
        } else {
            panic!("Expected PinnedSharded mode");
        }
    }

    #[test]
    fn test_runtime_mode_properties() {
        let single = RuntimeMode::SingleThread;
        assert_eq!(single.worker_count(), 1);
        assert!(!single.is_numa_aware());
        assert!(!single.supports_pinning());

        let pinned = RuntimeMode::PinnedSharded {
            worker_count: 4,
            numa_aware: true,
        };
        assert_eq!(pinned.worker_count(), 4);
        assert!(pinned.is_numa_aware());
        assert!(pinned.supports_pinning());

        let forced = RuntimeMode::ForcedSingle;
        assert_eq!(forced.worker_count(), 1);
        assert!(!forced.is_numa_aware());
        assert!(!forced.supports_pinning());

        let test_stub = RuntimeMode::TestStub;
        assert_eq!(test_stub.worker_count(), 1);
        assert!(!test_stub.is_numa_aware());
        assert!(!test_stub.supports_pinning());
    }

    #[test]
    fn test_runtime_mode_description() {
        let single = RuntimeMode::SingleThread;
        assert!(single.description().contains("Single-threaded"));

        let pinned = RuntimeMode::PinnedSharded {
            worker_count: 4,
            numa_aware: true,
        };
        let desc = pinned.description();
        assert!(desc.contains("Pinned sharded"));
        assert!(desc.contains("4 workers"));
        assert!(desc.contains("NUMA-aware: yes"));

        let forced = RuntimeMode::ForcedSingle;
        assert!(forced.description().contains("Forced single-threaded"));

        let test_stub = RuntimeMode::TestStub;
        assert!(test_stub.description().contains("Test stub"));
    }
}
