//! Cross-platform benchmark example
//!
//! This example compares performance between io_uring and fallback implementations,
//! running the appropriate implementation based on platform support.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::disk::{DiskConfig, DiskEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TEST_FILE_PATH: &str = "/tmp/elgate_benchmark_test_file";
const RING_PATH: &str = "/tmp/elgate_benchmark_ring";
const FILE_SIZE: usize = 100 * 1024 * 1024; // 100 MiB
const CHUNK_SIZE: usize = 64 * 1024; // 64 KiB
const ITERATIONS: usize = 10;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Elgate Cross-Platform I/O Benchmark");
    println!("===================================");

    // Detect CPU topology
    let cpu_info = CpuInfo::detect();
    println!(
        "Running on {} logical cores, {} NUMA nodes",
        cpu_info.logical_cores(),
        cpu_info.numa_nodes().len()
    );

    // Clean up from previous runs
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    // Create a test file with random data
    create_test_file(TEST_FILE_PATH, FILE_SIZE)?;

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH),
        size: 4 * 1024 * 1024, // 4 MiB
        slot_size: 64 * 1024,  // 64 KiB (larger for performance)
    };

    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create disk engine
    let config = DiskConfig {
        worker_threads: std::cmp::min(4, cpu_info.logical_cores() / 2),
        pin_threads: true,
        buffer_size: 128 * 1024, // 128 KiB
        numa_node: if cpu_info.is_numa_available() {
            Some(0)
        } else {
            None
        },
        #[cfg(feature = "io_uring")]
        queue_depth: 128,
    };

    let disk_engine = DiskEngine::new(config, ring.clone()).await?;

    // Print build configuration
    #[cfg(feature = "io_uring")]
    println!("Using io_uring implementation");
    #[cfg(not(feature = "io_uring"))]
    println!("Using fallback implementation");

    // Run read benchmark
    println!("\nSequential Read Benchmark:");
    let read_throughput =
        run_read_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, ITERATIONS).await?;
    println!("Average read throughput: {:.2} MB/s", read_throughput);

    // Run write benchmark
    println!("\nSequential Write Benchmark:");
    let write_throughput =
        run_write_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, ITERATIONS).await?;
    println!("Average write throughput: {:.2} MB/s", write_throughput);

    // Run random access benchmark
    println!("\nRandom Access Benchmark:");
    let random_throughput =
        run_random_access_benchmark(&disk_engine, TEST_FILE_PATH, CHUNK_SIZE, ITERATIONS).await?;
    println!(
        "Average random access throughput: {:.2} MB/s",
        random_throughput
    );

    // Clean up
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    Ok(())
}

fn create_test_file(path: &str, size: usize) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    let mut file = std::fs::File::create(path)?;
    let chunk_size = 1024 * 1024; // 1 MiB chunks for creation
    let mut remaining = size;

    // Generate some random data
    let mut rng = rand::thread_rng();
    let mut buffer = vec![0u8; chunk_size];

    while remaining > 0 {
        let to_write = std::cmp::min(remaining, chunk_size);

        // Fill buffer with random data
        for i in 0..to_write {
            buffer[i] = rand::random::<u8>();
        }

        file.write_all(&buffer[0..to_write])?;
        remaining -= to_write;
    }

    file.sync_all()?;
    Ok(())
}

async fn run_read_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_read = 0;

        for offset in (0..file_size).step_by(chunk_size) {
            let data = disk_engine.read_file(path, offset as u64).await?;
            total_read += data.len();
        }

        let elapsed = start.elapsed();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Read {} MiB in {:.2?} ({:.2} MB/s)",
            total_read / (1024 * 1024),
            elapsed,
            throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

async fn run_write_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        // Create a new path for each iteration to avoid caching effects
        let iter_path = format!("{}.{}", path, i);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_written = 0;

        // Generate data to write
        let data = vec![0x42; chunk_size]; // Fill with arbitrary data

        for offset in (0..file_size).step_by(chunk_size) {
            disk_engine
                .write_file(&iter_path, offset as u64, data.clone())
                .await?;
            total_written += chunk_size;
        }

        // Make sure data is committed to disk
        disk_engine.sync_file(&iter_path).await?;

        let elapsed = start.elapsed();
        let throughput = total_written as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Wrote {} MiB in {:.2?} ({:.2} MB/s)",
            total_written / (1024 * 1024),
            elapsed,
            throughput
        );

        // Clean up
        let _ = std::fs::remove_file(&iter_path);
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}

async fn run_random_access_benchmark(
    disk_engine: &DiskEngine,
    path: &str,
    chunk_size: usize,
    iterations: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let mut throughputs = Vec::new();

    for i in 0..iterations {
        println!("  Iteration {}/{}", i + 1, iterations);
        let start = Instant::now();
        let file_size = std::fs::metadata(path)?.len() as usize;
        let mut total_read = 0;

        // Generate random offsets
        let mut rng = rand::thread_rng();
        let max_offset = file_size.saturating_sub(chunk_size);
        let offsets: Vec<u64> = (0..100)
            .map(|_| rand::random::<u64>() % (max_offset as u64))
            .collect();

        for &offset in &offsets {
            let data = disk_engine.read_file(path, offset).await?;
            total_read += data.len();
        }

        let elapsed = start.elapsed();
        let operations_per_second = offsets.len() as f64 / elapsed.as_secs_f64();
        let throughput = total_read as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64();
        throughputs.push(throughput);
        println!(
            "    Performed {} random reads in {:.2?} ({:.2} ops/s, {:.2} MB/s)",
            offsets.len(),
            elapsed,
            operations_per_second,
            throughput
        );
    }

    Ok(throughputs.iter().sum::<f64>() / throughputs.len() as f64)
}
