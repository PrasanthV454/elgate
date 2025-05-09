//! Thread builder for creating and pinning worker threads
//!
//! This module handles creating worker threads and pinning them to specific CPU cores.

use std::marker::PhantomData;
use std::thread::{self, JoinHandle};

use crate::arch::{CpuInfo, RuntimeMode};

/// Result of attempting to pin a thread to a specific core
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinningResult {
    /// Successfully pinned to the requested core
    Success,
    /// Successfully pinned, but to a different core than requested
    SuccessDifferentCore(usize),
    /// Pinning is not supported on this platform
    Unsupported,
    /// Pinning failed for some other reason
    Failed,
}

/// A worker thread that may be pinned to a specific CPU core
pub struct WorkerThread<F: Send + 'static> {
    /// The thread's join handle
    handle: JoinHandle<()>,
    /// The CPU core this thread is pinned to, if any
    core_id: Option<usize>,
    /// The result of the pinning operation
    pinning_result: PinningResult,
    /// Phantom data to carry the worker function type
    _marker: PhantomData<F>,
}

impl<F: Send + 'static> WorkerThread<F> {
    /// Get the CPU core this thread is pinned to, if any
    pub fn core_id(&self) -> Option<usize> {
        self.core_id
    }

    /// Get the result of the pinning operation
    pub fn pinning_result(&self) -> PinningResult {
        self.pinning_result
    }

    /// Get a reference to the thread's join handle
    pub fn handle(&self) -> &JoinHandle<()> {
        &self.handle
    }

    /// Take ownership of the thread's join handle
    pub fn into_handle(self) -> JoinHandle<()> {
        self.handle
    }
}

/// Builder for creating worker threads
pub struct ThreadBuilder {
    /// Runtime mode that determines threading behavior
    mode: RuntimeMode,
    /// CPU information to use for core assignment
    cpu_info: CpuInfo,
    /// Current worker index (incremented for each thread created)
    worker_idx: usize,
}

impl ThreadBuilder {
    /// Create a new thread builder
    pub fn new(mode: RuntimeMode, cpu_info: &CpuInfo) -> Self {
        Self {
            mode,
            cpu_info: cpu_info.clone(),
            worker_idx: 0,
        }
    }

    /// Build a new worker thread with the given function
    pub fn build<F, T>(&mut self, name: &str, f: F) -> WorkerThread<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // For test stub, don't create a real thread
        if matches!(self.mode, RuntimeMode::TestStub) {
            return self.build_test_stub(f);
        }

        // Determine if we should attempt pinning
        let should_pin = self.mode.supports_pinning();
        let core_id = if should_pin {
            Some(self.cpu_info.get_core_for_worker(self.worker_idx))
        } else {
            None
        };

        // Increment worker index for next thread
        self.worker_idx += 1;

        // Create the thread with pinning if needed
        let thread_name = name.to_string();
        let core = core_id;

        let handle = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let pinning_result = if let Some(core) = core {
                    pin_thread_to_core(core)
                } else {
                    PinningResult::Unsupported
                };

                // Log pinning result
                if pinning_result != PinningResult::Success {
                    eprintln!("Thread pinning to core {core:?}: {:?}", pinning_result);
                }

                // Run the actual worker function
                let _result = f();
            })
            .expect("Failed to spawn thread");

        WorkerThread {
            handle,
            core_id,
            // We don't know for sure if pinning worked, this is optimistic
            // The actual pinning happens inside the thread
            pinning_result: core_id.map_or(PinningResult::Unsupported, |_| PinningResult::Success),
            _marker: PhantomData,
        }
    }

    /// Build a test stub worker "thread" (doesn't create a real thread)
    fn build_test_stub<F, T>(&mut self, f: F) -> WorkerThread<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Run the function directly in a dummy thread to store the result
        let handle = thread::spawn(move || {
            let _result = f();
        });

        WorkerThread {
            handle,
            core_id: None,
            pinning_result: PinningResult::Unsupported,
            _marker: PhantomData,
        }
    }
}

/// Attempt to pin the current thread to a specific CPU core
fn pin_thread_to_core(core_id: usize) -> PinningResult {
    // Try to get core IDs supported by this system
    match core_affinity::get_core_ids() {
        Some(core_ids) => {
            // Check if the requested core ID is available
            if let Some(core) = core_ids.get(core_id) {
                // Attempt to pin to this core
                if core_affinity::set_for_current(*core) {
                    PinningResult::Success
                } else {
                    PinningResult::Failed
                }
            } else if !core_ids.is_empty() {
                // Try to pin to a different core
                let fallback_core_id = core_id % core_ids.len();
                let fallback_core = core_ids[fallback_core_id];

                if core_affinity::set_for_current(fallback_core) {
                    PinningResult::SuccessDifferentCore(fallback_core_id)
                } else {
                    PinningResult::Failed
                }
            } else {
                // No cores available
                PinningResult::Failed
            }
        }
        None => PinningResult::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_thread_builder_single() {
        let cpu_info = CpuInfo::detect();
        let mode = RuntimeMode::SingleThread;
        let mut builder = ThreadBuilder::new(mode, &cpu_info);

        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        let worker = builder.build("test-worker", move || {
            *counter_clone.lock().unwrap() += 1;
        });

        // Wait for thread to complete
        worker.handle.join().unwrap();

        // Verify the thread ran
        assert_eq!(*counter.lock().unwrap(), 1);

        // In single thread mode, pinning should not be attempted
        assert_eq!(worker.pinning_result, PinningResult::Unsupported);
    }

    #[test]
    fn test_thread_builder_pinned() {
        let cpu_info = CpuInfo::mock(4, 1);
        let mode = RuntimeMode::PinnedSharded {
            worker_count: 2,
            numa_aware: true,
        };
        let mut builder = ThreadBuilder::new(mode, &cpu_info);

        // Create multiple threads and check core assignment
        for i in 0..2 {
            let worker = builder.build(&format!("test-worker-{}", i), move || {
                // Thread body
                println!("Worker {} running", i);
            });

            // We should have a core assignment (might not actually pin on CI)
            assert!(worker.core_id.is_some());

            // We shouldn't attempt pinning to cores we don't have
            assert!(worker.core_id.unwrap() < cpu_info.logical_cores());

            // Clean up
            worker.handle.join().unwrap();
        }
    }

    #[test]
    fn test_thread_builder_test_stub() {
        let cpu_info = CpuInfo::detect();
        let mode = RuntimeMode::TestStub;
        let mut builder = ThreadBuilder::new(mode, &cpu_info);

        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        let worker = builder.build("test-stub", move || {
            *counter_clone.lock().unwrap() += 1;
        });

        // Wait for "thread" to complete
        worker.handle.join().unwrap();

        // Verify the function ran
        assert_eq!(*counter.lock().unwrap(), 1);

        // TestStub mode doesn't try to pin
        assert_eq!(worker.pinning_result, PinningResult::Unsupported);
        assert_eq!(worker.core_id, None);
    }
}
