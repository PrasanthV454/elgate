//! Thread Builder example for Elgate.
//!
//! This example demonstrates how to use the thread builder for
//! creating CPU and NUMA-aware threads.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::arch::thread_builder::ThreadBuilder;
use elgate_core::arch::RuntimeMode;
use std::sync::Arc;
use std::thread;

fn main() -> anyhow::Result<()> {
    // Detect CPU topology
    let cpu_info = CpuInfo::detect();

    println!("CPU Information:");
    println!("  Total CPUs: {}", cpu_info.logical_cores());
    println!("  Physical cores: {:?}", cpu_info.physical_cores());
    println!("  Logical cores: {}", cpu_info.logical_cores());

    if cpu_info.is_numa_available() {
        println!("  NUMA nodes: {}", cpu_info.numa_nodes().len());
        for (i, node) in cpu_info.numa_nodes().iter().enumerate() {
            println!("    Node {}: CPUs {:?}", i, node);
        }
    } else {
        println!("  NUMA: not available");
    }

    // Choose a runtime mode that supports pinning
    let pinned_mode = || RuntimeMode::PinnedSharded {
        worker_count: 2,
        numa_aware: true,
    };

    // Example 1: Create a thread with specific CPU affinity
    println!("\nExample 1: Thread with CPU affinity");

    let mut builder = ThreadBuilder::new(pinned_mode(), &cpu_info);
    let worker = builder.build("affinity-thread", || {
        println!("Thread started with CPU affinity");
        println!("Running computation on pinned core...");

        // Do some work
        let iterations = 10_000_000;
        let start = std::time::Instant::now();

        let mut sum: u64 = 0;
        for i in 0..iterations {
            sum = sum.wrapping_add(i);
        }

        let duration = start.elapsed();
        println!("Computed sum {} in {:?}", sum, duration);

        // Print pinning information
        if let Some(core_id) = thread::current().name().and(None::<usize>) {
            println!("Thread is running on CPU {}", core_id);
        } else {
            println!("Could not determine current core ID");
        }
    });

    // Get the information we need before moving it
    let pinning_result = worker.pinning_result();
    let core_id = worker.core_id();

    // Wait for thread to complete
    worker.into_handle().join().unwrap();

    // Use the saved variables instead of accessing worker:
    println!("Thread pinning result: {:?}", pinning_result);
    if let Some(core_id) = core_id {
        println!("Thread was assigned to core {}", core_id);
    }

    // Example 2: Create multiple threads (some will be on different NUMA nodes if available)
    if cpu_info.is_numa_available() && !cpu_info.numa_nodes().is_empty() {
        println!("\nExample 2: Multiple threads with NUMA awareness");

        let mut builder = ThreadBuilder::new(pinned_mode(), &cpu_info);
        let worker_count = 4;
        let mut workers = Vec::new();

        for i in 0..worker_count {
            let worker = builder.build(&format!("worker-{}", i), move || {
                println!("Worker {} started", i);

                // Do some memory-intensive work
                let mb = 10; // Allocate 10 MB
                let vec_size = mb * 1024 * 1024 / std::mem::size_of::<u64>();

                let start = std::time::Instant::now();

                // Allocate memory (should be local to the NUMA node)
                let mut data = Vec::<u64>::with_capacity(vec_size);

                // Fill and access the memory
                for j in 0..vec_size {
                    data.push(j as u64);
                }

                // Access the memory
                let mut sum: u64 = 0;
                for val in &data {
                    sum = sum.wrapping_add(*val);
                }

                let duration = start.elapsed();
                println!("Worker {}: computed sum {} in {:?}", i, sum, duration);
            });

            workers.push(worker);
        }

        // Wait for all workers to complete
        for (i, worker) in workers.into_iter().enumerate() {
            worker.into_handle().join().unwrap();
            println!("Worker {} completed", i);
        }
    } else {
        println!("\nSkipping NUMA-aware thread example as NUMA is not available");
    }

    // Example 3: Test Stub mode (doesn't create real threads)
    println!("\nExample 3: Test Stub mode");

    let mut builder = ThreadBuilder::new(RuntimeMode::TestStub, &cpu_info);
    let counter = Arc::new(std::sync::Mutex::new(0));
    let counter_clone = counter.clone();

    let worker = builder.build("test-stub", move || {
        println!("Test stub worker running");
        *counter_clone.lock().unwrap() += 1;
    });

    // Get the information we need before moving it
    let pinning_result = worker.pinning_result();

    // Wait for "thread" to complete
    worker.into_handle().join().unwrap();

    // Use the saved variable:
    println!("Test stub counter: {}", *counter.lock().unwrap());
    println!("Test stub pinning result: {:?}", pinning_result);

    println!("\nThread builder example completed successfully!");
    Ok(())
}
