//! Performance benchmarking example for Elgate.
//!
//! This example benchmarks the performance of the ring buffer, disk I/O,
//! and network I/O components.

use elgate_core::arch::cpu_info::CpuInfo;
#[cfg(feature = "io_uring")]
use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
#[cfg(feature = "io_uring")]
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{self, OperationKind, RingBufferOptions};

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const KB: usize = 1024;
const MB: usize = 1024 * KB;
const ITERATIONS: usize = 10_000;
const DATA_SIZE: usize = 4 * KB; // 4 KiB per operation
const TOTAL_DATA: usize = ITERATIONS * DATA_SIZE; // Total data transferred

// Benchmark the ring buffer performance
async fn benchmark_ring_buffer() {
    println!("\n=== Ring Buffer Performance ===");

    // Create a test-specific path
    let test_path = "/tmp/elgate_bench_ring";
    let _ = fs::remove_file(test_path);

    // Create ring buffer
    let options = RingBufferOptions {
        path: PathBuf::from(test_path),
        size: 16 * MB,     // 16 MiB
        slot_size: 8 * KB, // 8 KiB
    };

    let ring = Arc::new(ring::RingBuffer::create(options.clone()).unwrap());
    let consumer_ring = Arc::new(ring::RingBuffer::open(options).unwrap());

    // Prepare test data
    let data = vec![0x42; DATA_SIZE];

    // Measure producer throughput
    let start = Instant::now();

    for _ in 0..ITERATIONS {
        ring.write(OperationKind::DiskWrite, &data).unwrap();
    }

    let producer_elapsed = start.elapsed();
    let producer_throughput = (TOTAL_DATA as f64) / (producer_elapsed.as_secs_f64() * MB as f64);

    println!(
        "Producer: {} iterations in {:.2?}",
        ITERATIONS, producer_elapsed
    );
    println!("Producer throughput: {:.2} MB/s", producer_throughput);

    // Measure consumer throughput
    let start = Instant::now();

    let mut read_count = 0;
    while read_count < ITERATIONS {
        if let Some(_) = consumer_ring.read().unwrap() {
            read_count += 1;
        }
    }

    let consumer_elapsed = start.elapsed();
    let consumer_throughput = (TOTAL_DATA as f64) / (consumer_elapsed.as_secs_f64() * MB as f64);

    println!(
        "Consumer: {} iterations in {:.2?}",
        ITERATIONS, consumer_elapsed
    );
    println!("Consumer throughput: {:.2} MB/s", consumer_throughput);

    // Clean up
    let _ = fs::remove_file(test_path);
}

// Benchmark the disk I/O performance
#[cfg(feature = "io_uring")]
async fn benchmark_disk_io() {
    println!("\n=== Disk I/O Performance ===");

    // Create paths for testing
    let ring_path = "/tmp/elgate_bench_disk_ring";
    let test_file_path = "/tmp/elgate_bench_disk_file";

    // Clean up from previous runs
    let _ = fs::remove_file(ring_path);
    let _ = fs::remove_file(test_file_path);

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 16 * MB,     // 16 MiB
        slot_size: 8 * KB, // 8 KiB
    };

    let ring = Arc::new(ring::RingBuffer::create(ring_options).unwrap());

    // Create disk engine
    let config = DiskConfig {
        worker_threads: 2,
        pin_threads: true,
        queue_depth: 128,
        buffer_size: 128 * KB,
        numa_node: None,
    };

    let disk_engine = DiskEngine::new(config, ring.clone()).await.unwrap();

    // Prepare test data
    let data = vec![0x42; DATA_SIZE];

    // Measure write throughput
    println!(
        "Writing {} MB in {} KB chunks...",
        TOTAL_DATA / MB,
        DATA_SIZE / KB
    );
    let start = Instant::now();

    for i in 0..ITERATIONS {
        let offset = i * DATA_SIZE;
        disk_engine
            .write_file(test_file_path, offset as u64, data.clone())
            .await
            .unwrap();
    }

    let write_elapsed = start.elapsed();
    let write_throughput = (TOTAL_DATA as f64) / (write_elapsed.as_secs_f64() * MB as f64);

    println!("Write: {} iterations in {:.2?}", ITERATIONS, write_elapsed);
    println!("Write throughput: {:.2} MB/s", write_throughput);

    // Measure read throughput
    println!(
        "Reading {} MB in {} KB chunks...",
        TOTAL_DATA / MB,
        DATA_SIZE / KB
    );
    let start = Instant::now();

    for i in 0..ITERATIONS {
        let offset = i * DATA_SIZE;
        let _ = disk_engine
            .read_file(test_file_path, offset as u64)
            .await
            .unwrap();
    }

    let read_elapsed = start.elapsed();
    let read_throughput = (TOTAL_DATA as f64) / (read_elapsed.as_secs_f64() * MB as f64);

    println!("Read: {} iterations in {:.2?}", ITERATIONS, read_elapsed);
    println!("Read throughput: {:.2} MB/s", read_throughput);

    // Clean up
    let _ = fs::remove_file(ring_path);
    let _ = fs::remove_file(test_file_path);
}

#[cfg(not(feature = "io_uring"))]
async fn benchmark_disk_io() {
    println!("Disk I/O benchmark skipped: io_uring feature not enabled");
}

// Benchmark the network I/O performance
async fn benchmark_network_io() {
    println!("\n=== Network I/O Performance ===");

    // Skip if io_uring feature is not enabled
    if !cfg!(feature = "io_uring") {
        println!("Skipping network I/O benchmark (io_uring feature not enabled)");
        return;
    }

    // Create a path for the ring buffer
    let ring_path = "/tmp/elgate_bench_net_ring";

    // Clean up from previous runs
    let _ = fs::remove_file(ring_path);

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(ring_path),
        size: 16 * MB,     // 16 MiB
        slot_size: 8 * KB, // 8 KiB
    };

    let ring = Arc::new(ring::RingBuffer::create(ring_options).unwrap());

    // Create network engine
    let config = NetworkConfig {
        worker_threads: 2,
        pin_threads: true,
        queue_depth: 128,
        buffer_size: 128 * KB,
        numa_node: None,
    };

    let net_engine = NetworkEngine::new(config, ring.clone()).await.unwrap();

    // Start echo server in a separate thread
    let server_addr = "127.0.0.1:9999".parse::<SocketAddr>().unwrap();
    let server_thread = thread::spawn(move || {
        let listener = TcpListener::bind(server_addr).unwrap();
        println!("Echo server listening on {}", server_addr);

        let (mut stream, _) = listener.accept().unwrap();
        println!("Client connected");

        let mut buffer = vec![0u8; DATA_SIZE];
        for _ in 0..ITERATIONS {
            let n = stream.read(&mut buffer).unwrap();
            stream.write_all(&buffer[0..n]).unwrap();
        }
    });

    // Give the server time to start
    thread::sleep(Duration::from_millis(100));

    // Connect to the server
    let socket_fd = net_engine.connect(server_addr).await.unwrap();
    println!("Connected to echo server");

    // Prepare test data
    let data = vec![0x42; DATA_SIZE];

    // Measure throughput for round-trip (write then read)
    println!(
        "Sending/receiving {} MB in {} KB chunks...",
        (TOTAL_DATA * 2) / MB,
        DATA_SIZE / KB
    );
    let start = Instant::now();

    for _ in 0..ITERATIONS {
        // Write data
        let _bytes_written = net_engine.write(socket_fd, data.clone()).await.unwrap();

        // Read echo response
        let _read_data = net_engine.read(socket_fd).await.unwrap();
    }

    let elapsed = start.elapsed();
    // Double the data size for round-trip calculation
    let throughput = (TOTAL_DATA as f64 * 2.0) / (elapsed.as_secs_f64() * MB as f64);

    println!("Round-trip: {} iterations in {:.2?}", ITERATIONS, elapsed);
    println!("Round-trip throughput: {:.2} MB/s", throughput);

    // Close the socket
    net_engine.close(socket_fd).await.unwrap();

    // Wait for the server to complete
    server_thread.join().unwrap();

    // Clean up
    let _ = fs::remove_file(ring_path);
}

// Benchmark NUMA-aware ring buffer performance
async fn benchmark_numa_ring_buffer() {
    println!("\n=== NUMA-Aware Ring Buffer Performance ===");

    // Get CPU info
    let cpu_info = CpuInfo::detect();

    if !cpu_info.is_numa_available() || cpu_info.numa_nodes().len() <= 1 {
        println!("Skipping NUMA benchmark (NUMA not available or only one node)");
        return;
    }

    println!("NUMA available with {} nodes", cpu_info.numa_nodes().len());

    // Create a test-specific path
    let test_path_regular = "/tmp/elgate_bench_ring_regular";
    let test_path_numa = "/tmp/elgate_bench_ring_numa";

    let _ = fs::remove_file(test_path_regular);
    let _ = fs::remove_file(test_path_numa);

    // Create options
    let options_regular = RingBufferOptions {
        path: PathBuf::from(test_path_regular),
        size: 16 * MB,     // 16 MiB
        slot_size: 8 * KB, // 8 KiB
    };

    let options_numa = RingBufferOptions {
        path: PathBuf::from(test_path_numa),
        size: 16 * MB,     // 16 MiB
        slot_size: 8 * KB, // 8 KiB
    };

    // Create regular ring buffer
    println!("Testing regular ring buffer...");
    let ring_regular = Arc::new(ring::RingBuffer::create(options_regular.clone()).unwrap());

    // Create NUMA-aware ring buffer
    println!("Testing NUMA-aware ring buffer on node 0...");
    let ring_numa = Arc::new(ring::RingBuffer::create_numa_aware(options_numa.clone(), 0).unwrap());

    // Prepare test data
    let data = vec![0x42; DATA_SIZE];

    // Benchmark regular ring buffer
    let start = Instant::now();

    for _ in 0..ITERATIONS {
        ring_regular.write(OperationKind::DiskWrite, &data).unwrap();
    }

    let regular_elapsed = start.elapsed();
    let regular_throughput = (TOTAL_DATA as f64) / (regular_elapsed.as_secs_f64() * MB as f64);

    println!(
        "Regular: {} iterations in {:.2?}",
        ITERATIONS, regular_elapsed
    );
    println!("Regular throughput: {:.2} MB/s", regular_throughput);

    // Benchmark NUMA-aware ring buffer
    let start = Instant::now();

    for _ in 0..ITERATIONS {
        ring_numa.write(OperationKind::DiskWrite, &data).unwrap();
    }

    let numa_elapsed = start.elapsed();
    let numa_throughput = (TOTAL_DATA as f64) / (numa_elapsed.as_secs_f64() * MB as f64);

    println!(
        "NUMA-aware: {} iterations in {:.2?}",
        ITERATIONS, numa_elapsed
    );
    println!("NUMA-aware throughput: {:.2} MB/s", numa_throughput);
    println!(
        "Improvement: {:.2}%",
        (numa_throughput / regular_throughput - 1.0) * 100.0
    );

    // Clean up
    let _ = fs::remove_file(test_path_regular);
    let _ = fs::remove_file(test_path_numa);
}

#[tokio::main]
async fn main() {
    println!("Running performance benchmarks for Elgate components...");

    // Print CPU information
    let cpu_info = CpuInfo::detect();
    println!(
        "CPU: {} cores ({} physical, {} logical)",
        cpu_info.logical_cores(),
        cpu_info.physical_cores().unwrap_or(0),
        cpu_info.logical_cores()
    );

    if cpu_info.is_numa_available() {
        println!("NUMA: {} nodes", cpu_info.numa_nodes().len());
    } else {
        println!("NUMA: not available");
    }

    // Run benchmarks
    benchmark_ring_buffer().await;
    benchmark_disk_io().await;
    benchmark_network_io().await;
    benchmark_numa_ring_buffer().await;

    println!("\nAll benchmarks completed!");
}
