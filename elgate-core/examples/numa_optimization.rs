//! NUMA optimization example
//!
//! This example demonstrates the performance benefits of NUMA-aware operations
//! by comparing NUMA-optimized and non-optimized ring buffers.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::ring::{OperationKind, RingBuffer, RingBufferOptions};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const OPERATIONS_PER_THREAD: usize = 100_000;
const PAYLOAD_SIZE: usize = 1024; // 1 KiB
const NUM_THREADS: usize = 4;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Detect CPU topology
    let cpu_info = CpuInfo::detect();
    println!("Detected {} logical cores", cpu_info.logical_cores());
    println!("Detected {} NUMA nodes", cpu_info.numa_nodes().len());

    // Skip detailed test if NUMA is not available
    if !cpu_info.is_numa_available() || cpu_info.numa_nodes().len() < 2 {
        println!("This system doesn't have multiple NUMA nodes available.");
        println!("Running simplified benchmark without NUMA optimization.");

        // Run simplified benchmark
        run_simplified_benchmark()?;
        return Ok(());
    }

    println!("Running NUMA optimization benchmark...");

    // Create standard ring buffer
    let standard_ring_path = "/tmp/elgate_standard_ring";
    let _ = std::fs::remove_file(standard_ring_path);

    let standard_options = RingBufferOptions {
        path: PathBuf::from(standard_ring_path),
        size: 16 * 1024 * 1024, // 16 MiB
        slot_size: 4096,        // 4 KiB
    };

    let standard_ring = Arc::new(RingBuffer::create(standard_options)?);

    // Create NUMA-aware ring buffer (node 0)
    let numa_ring_path = "/tmp/elgate_numa_ring";
    let _ = std::fs::remove_file(numa_ring_path);

    let numa_options = RingBufferOptions {
        path: PathBuf::from(numa_ring_path),
        size: 16 * 1024 * 1024, // 16 MiB
        slot_size: 4096,        // 4 KiB
    };

    let numa_ring = Arc::new(RingBuffer::create_numa_aware(numa_options, 0)?);

    // Prepare test data
    let test_data = vec![0x42; PAYLOAD_SIZE];

    // Run standard benchmark
    println!("\nBenchmarking standard ring buffer...");
    let standard_time = run_benchmark(standard_ring.clone(), &test_data, NUM_THREADS)?;

    // Run NUMA benchmark
    println!("\nBenchmarking NUMA-optimized ring buffer...");
    let numa_time = run_benchmark(numa_ring.clone(), &test_data, NUM_THREADS)?;

    // Calculate improvement
    let improvement = (standard_time.as_secs_f64() / numa_time.as_secs_f64() - 1.0) * 100.0;

    println!("\nResults:");
    println!("  Standard Ring: {:.2?}", standard_time);
    println!("  NUMA Ring:     {:.2?}", numa_time);
    println!("  Improvement:   {:.2}%", improvement);

    // Clean up
    let _ = std::fs::remove_file(standard_ring_path);
    let _ = std::fs::remove_file(numa_ring_path);

    Ok(())
}

fn run_simplified_benchmark() -> Result<(), Box<dyn std::error::Error>> {
    // Create standard ring buffer
    let ring_path = "/tmp/elgate_ring";
    let _ = std::fs::remove_file(ring_path);

    let options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 16 * 1024 * 1024, // 16 MiB
        slot_size: 4096,        // 4 KiB
    };

    let ring = Arc::new(RingBuffer::create(options)?);

    // Prepare test data
    let test_data = vec![0x42; PAYLOAD_SIZE];

    // Run benchmark
    println!("\nBenchmarking ring buffer...");
    let time = run_benchmark(ring.clone(), &test_data, 2)?;

    println!("\nResults:");
    println!("  Total time: {:.2?}", time);
    println!("  Operations: {}", OPERATIONS_PER_THREAD * 2);
    println!(
        "  Throughput: {:.2} ops/s",
        (OPERATIONS_PER_THREAD * 2) as f64 / time.as_secs_f64()
    );

    // Clean up
    let _ = std::fs::remove_file(ring_path);

    Ok(())
}

fn run_benchmark(
    ring: Arc<RingBuffer>,
    test_data: &[u8],
    num_threads: usize,
) -> Result<Duration, Box<dyn std::error::Error>> {
    let start = Instant::now();

    // Spawn writer threads
    let mut handles = Vec::with_capacity(num_threads);

    for thread_idx in 0..num_threads {
        let ring_clone = ring.clone();
        let test_data = test_data.to_vec();

        let handle = thread::spawn(move || -> Result<(), Box<dyn std::error::Error + Send>> {
            for i in 0..OPERATIONS_PER_THREAD {
                // Write to ring buffer
                ring_clone.write(OperationKind::Custom, &test_data)?;

                // Every 10,000 operations, print progress
                if i > 0 && i % 10000 == 0 {
                    println!("Thread {}: {} operations completed", thread_idx, i);
                }
            }

            println!("Thread {} completed all operations", thread_idx);
            Ok(())
        });

        handles.push(handle);
    }

    // Spawn reader thread
    let reader_ring = ring.clone();
    let reader_handle: JoinHandle<Result<usize, anyhow::Error>> = thread::spawn(move || {
        let mut read_count = 0;
        let total_expected = OPERATIONS_PER_THREAD * num_threads;

        while read_count < total_expected {
            // Read from ring buffer
            match reader_ring.read() {
                Ok(Some(_)) => {
                    read_count += 1;

                    // Every 10,000 operations, print progress
                    if read_count > 0 && read_count % 10000 == 0 {
                        println!("Reader: {} operations read", read_count);
                    }
                }
                Ok(None) => {
                    // No data available, sleep briefly
                    std::thread::sleep(Duration::from_micros(100));
                }
                Err(e) => {
                    return Err(Box::new(e));
                }
            }
        }

        println!("Reader completed: {} operations read", read_count);
        Ok(read_count)
    });

    // Wait for all writer threads
    for (i, handle) in handles.into_iter().enumerate() {
        if let Err(e) = handle.join().unwrap() {
            eprintln!("Thread {} error: {}", i, e);
        }
    }

    // Wait for reader thread
    let read_count = reader_handle.join().unwrap()?;
    println!("Total operations read: {}", read_count);

    let elapsed = start.elapsed();
    let throughput = (OPERATIONS_PER_THREAD * num_threads) as f64 / elapsed.as_secs_f64();
    println!("Throughput: {:.2} ops/s", throughput);

    Ok(elapsed)
}
