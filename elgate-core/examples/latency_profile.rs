//! Latency profiling example
//!
//! This example profiles the latency of different I/O operations
//! to help identify bottlenecks and optimize performance.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::disk::io_uring::{DiskConfig, DiskEngine};
use elgate_core::net::io_uring::{NetworkConfig, NetworkEngine};
use elgate_core::ring::{RingBuffer, RingBufferOptions};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TEST_FILE_PATH: &str = "/tmp/elgate_latency_test_file";
const RING_PATH: &str = "/tmp/elgate_latency_ring";
const NUM_OPERATIONS: usize = 1000;

#[derive(Debug, Clone)]
struct LatencyStats {
    min: Duration,
    max: Duration,
    total: Duration,
    count: usize,
    p50: Duration,
    p90: Duration,
    p99: Duration,
}

impl LatencyStats {
    fn new(samples: &[Duration]) -> Self {
        if samples.is_empty() {
            return Self {
                min: Duration::from_secs(0),
                max: Duration::from_secs(0),
                total: Duration::from_secs(0),
                count: 0,
                p50: Duration::from_secs(0),
                p90: Duration::from_secs(0),
                p99: Duration::from_secs(0),
            };
        }

        let mut sorted = samples.to_vec();
        sorted.sort();

        let min = sorted[0];
        let max = sorted[sorted.len() - 1];
        let total = sorted.iter().sum();

        let p50_idx = (sorted.len() as f64 * 0.5) as usize;
        let p90_idx = (sorted.len() as f64 * 0.9) as usize;
        let p99_idx = (sorted.len() as f64 * 0.99) as usize;

        Self {
            min,
            max,
            total,
            count: samples.len(),
            p50: sorted[p50_idx],
            p90: sorted[p90_idx],
            p99: sorted[p99_idx],
        }
    }

    fn average(&self) -> Duration {
        if self.count == 0 {
            return Duration::from_secs(0);
        }

        Duration::from_nanos((self.total.as_nanos() / self.count as u128) as u64)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Elgate Latency Profiling");
    println!("=======================");

    // Detect CPU topology
    let cpu_info = CpuInfo::detect();
    println!("Running on {} logical cores", cpu_info.logical_cores());

    // Clean up from previous runs
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    // Create test file
    create_test_file(TEST_FILE_PATH, 10 * 1024 * 1024)?; // 10 MiB file

    // Create ring buffer
    let ring_options = RingBufferOptions {
        path: PathBuf::from(RING_PATH),
        size: 16 * 1024 * 1024, // 16 MiB
        slot_size: 64 * 1024,   // 64 KiB
    };

    let ring = Arc::new(RingBuffer::create(ring_options)?);

    // Create disk engine
    let disk_config = DiskConfig {
        worker_threads: 2,
        pin_threads: false, // Don't pin in example for compatibility
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 64,
    };

    let disk_engine = DiskEngine::new(disk_config, ring.clone()).await?;

    // Create network engine
    let net_config = NetworkConfig {
        worker_threads: 2,
        pin_threads: false,
        buffer_size: 64 * 1024,
        numa_node: None,
        #[cfg(feature = "io_uring")]
        queue_depth: 64,
    };

    let net_engine = NetworkEngine::new(net_config, ring).await?;

    // Start network server
    let (server_handle, server_running) = start_network_server().await?;

    // Run disk read latency test
    println!("\nProfiling disk read latency...");
    let read_latencies = profile_disk_read(&disk_engine, TEST_FILE_PATH, NUM_OPERATIONS).await?;
    let read_stats = LatencyStats::new(&read_latencies);

    // Run disk write latency test
    println!("\nProfiling disk write latency...");
    let write_latencies = profile_disk_write(&disk_engine, TEST_FILE_PATH, NUM_OPERATIONS).await?;
    let write_stats = LatencyStats::new(&write_latencies);

    // Run network latency test
    println!("\nProfiling network latency...");
    let network_latencies = profile_network(&net_engine, NUM_OPERATIONS / 10).await?;
    let network_stats = LatencyStats::new(&network_latencies);

    // Stop network server
    server_running.store(false, Ordering::SeqCst);
    let _ = server_handle.await;

    // Print results
    println!("\nLatency Profile Results");
    println!("======================");

    print_stats("Disk Read", &read_stats);
    print_stats("Disk Write", &write_stats);
    print_stats("Network Roundtrip", &network_stats);

    // Compare io_uring vs fallback if relevant
    #[cfg(feature = "io_uring")]
    println!("\nUsing io_uring implementation");

    #[cfg(not(feature = "io_uring"))]
    println!("\nUsing fallback implementation");

    // Generate latency histograms
    println!("\nLatency Histograms");
    println!("=================");

    println!("\nDisk Read Latency Histogram (microseconds):");
    print_histogram(&read_latencies);

    println!("\nDisk Write Latency Histogram (microseconds):");
    print_histogram(&write_latencies);

    println!("\nNetwork Latency Histogram (microseconds):");
    print_histogram(&network_latencies);

    // Clean up
    let _ = std::fs::remove_file(TEST_FILE_PATH);
    let _ = std::fs::remove_file(RING_PATH);

    Ok(())
}

fn create_test_file(path: &str, size: usize) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)?;
    let data = vec![0x42; size]; // Fill with arbitrary data

    file.write_all(&data)?;
    file.sync_all()?;
    Ok(())
}

fn print_stats(name: &str, stats: &LatencyStats) {
    println!("\n{} Latency:", name);
    println!("  Min:     {:.2?}", stats.min);
    println!("  Max:     {:.2?}", stats.max);
    println!("  Average: {:.2?}", stats.average());
    println!("  p50:     {:.2?}", stats.p50);
    println!("  p90:     {:.2?}", stats.p90);
    println!("  p99:     {:.2?}", stats.p99);
    println!("  Count:   {}", stats.count);
}

fn print_histogram(samples: &[Duration]) {
    // Convert to microseconds for more readable output
    let micros: Vec<u64> = samples.iter().map(|d| d.as_micros() as u64).collect();

    if micros.is_empty() {
        println!("  No samples collected");
        return;
    }

    // Create histogram buckets (logarithmic scale)
    let mut histogram = BTreeMap::new();
    let mut min = u64::MAX;
    let mut max = 0;

    for &us in &micros {
        min = min.min(us);
        max = max.max(us);

        // Create logarithmic buckets
        let bucket = if us < 10 {
            us
        } else if us < 100 {
            (us / 10) * 10
        } else if us < 1000 {
            (us / 100) * 100
        } else if us < 10000 {
            (us / 1000) * 1000
        } else {
            (us / 10000) * 10000
        };

        *histogram.entry(bucket).or_insert(0) += 1;
    }

    // Print histogram
    let max_count = histogram.values().cloned().max().unwrap_or(0);
    let width = 50; // Width of the histogram bar

    for (bucket, count) in histogram {
        let bucket_label = match bucket {
            b if b < 10 => format!("{}μs", b),
            b if b < 100 => format!("{}μs", b),
            b if b < 1000 => format!("{}μs", b),
            b if b < 10000 => format!("{:.1}ms", b as f64 / 1000.0),
            _ => format!("{:.1}ms", bucket as f64 / 1000.0),
        };

        let bar_length = (count * width) / max_count;
        let bar = "#".repeat(bar_length as usize);

        println!("  {:>8} | {:3} | {}", bucket_label, count, bar);
    }
}

async fn start_network_server(
) -> Result<(tokio::task::JoinHandle<()>, Arc<AtomicBool>), Box<dyn std::error::Error>> {
    // Flag to signal when to stop the server
    let server_running = Arc::new(AtomicBool::new(true));
    let server_running_clone = server_running.clone();

    // Start server in a separate task
    let handle = tokio::spawn(async move {
        let addr = "127.0.0.1:7777".parse::<SocketAddr>().unwrap();
        let listener = TcpListener::bind(addr).unwrap();
        listener.set_nonblocking(true).unwrap();

        println!("Latency test server listening on {}", addr);

        // Use a standard socket for the echo server
        let listener_fd = listener.as_raw_fd();

        'outer: while server_running_clone.load(Ordering::SeqCst) {
            // Accept connection using standard sockets for simplicity
            match listener.accept() {
                Ok((mut socket, addr)) => {
                    println!("Accepted connection from {}", addr);

                    // Echo server - read data and send it back
                    let mut buf = [0u8; 4096];

                    while server_running_clone.load(Ordering::SeqCst) {
                        match socket.set_nonblocking(true) {
                            Ok(_) => {}
                            Err(e) => {
                                eprintln!("Failed to set nonblocking: {}", e);
                                break;
                            }
                        }

                        match socket.read(&mut buf) {
                            Ok(0) => break, // End of stream
                            Ok(n) => {
                                // Echo data back
                                if let Err(e) = socket.write_all(&buf[0..n]) {
                                    eprintln!("Write error: {}", e);
                                    break;
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // No data available, sleep briefly
                                std::thread::sleep(Duration::from_millis(1));
                                continue;
                            }
                            Err(e) => {
                                eprintln!("Read error: {}", e);
                                break;
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No connection available, sleep briefly
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => {
                    eprintln!("Accept error: {}", e);
                    break 'outer;
                }
            }
        }

        println!("Latency test server stopped");
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((handle, server_running))
}

async fn profile_disk_read(
    disk_engine: &DiskEngine,
    path: &str,
    count: usize,
) -> Result<Vec<Duration>, Box<dyn std::error::Error>> {
    let mut latencies = Vec::with_capacity(count);

    // Warm up (not included in measurements)
    for _ in 0..10 {
        let _ = disk_engine.read_file(path, 0).await?;
    }

    println!("Running {} disk read operations...", count);
    for i in 0..count {
        // Generate random offset within file size
        let file_size = std::fs::metadata(path)?.len() as u64;
        let offset = rand::random::<u64>() % file_size;

        // Measure latency
        let start = Instant::now();
        let _ = disk_engine.read_file(path, offset).await?;
        let elapsed = start.elapsed();

        latencies.push(elapsed);

        // Print progress
        if (i + 1) % 100 == 0 || i + 1 == count {
            println!("  Progress: {}/{}", i + 1, count);
        }
    }

    Ok(latencies)
}

async fn profile_disk_write(
    disk_engine: &DiskEngine,
    path: &str,
    count: usize,
) -> Result<Vec<Duration>, Box<dyn std::error::Error>> {
    let mut latencies = Vec::with_capacity(count);
    let data = vec![0x42; 4096]; // 4 KiB of data

    // Create a test path
    let test_path = format!("{}.write", path);

    // Warm up (not included in measurements)
    for _ in 0..10 {
        let _ = disk_engine.write_file(&test_path, 0, data.clone()).await?;
    }

    println!("Running {} disk write operations...", count);
    for i in 0..count {
        // Generate random offset
        let max_offset = 1024 * 1024 * 1024; // 1 GiB max
        let offset = rand::random::<u64>() % max_offset;

        // Measure latency
        let start = Instant::now();
        let _ = disk_engine
            .write_file(&test_path, offset, data.clone())
            .await?;
        let elapsed = start.elapsed();

        latencies.push(elapsed);

        // Print progress
        if (i + 1) % 100 == 0 || i + 1 == count {
            println!("  Progress: {}/{}", i + 1, count);
        }

        // Occasionally sync to disk
        if (i + 1) % 100 == 0 {
            let _ = disk_engine.sync_file(&test_path).await?;
        }
    }

    // Final sync
    let _ = disk_engine.sync_file(&test_path).await?;

    // Clean up
    let _ = std::fs::remove_file(&test_path);

    Ok(latencies)
}

async fn profile_network(
    net_engine: &NetworkEngine,
    count: usize,
) -> Result<Vec<Duration>, Box<dyn std::error::Error>> {
    let mut latencies = Vec::with_capacity(count);
    let data = vec![0x42; 1024]; // 1 KiB of data

    // Connect to echo server
    let addr = "127.0.0.1:7777".parse::<SocketAddr>()?;
    let fd = net_engine.connect(addr).await?;
    println!("Connected to echo server at {}", addr);

    // Warm up (not included in measurements)
    for _ in 0..5 {
        let _ = net_engine.write(fd, data.clone()).await?;
        let _ = net_engine.read(fd).await?;
    }

    println!("Running {} network roundtrip operations...", count);
    for i in 0..count {
        // Measure latency of write+read (roundtrip)
        let start = Instant::now();

        // Write data
        let bytes_written = net_engine.write(fd, data.clone()).await?;

        // Read response
        let response = net_engine.read(fd).await?;

        let elapsed = start.elapsed();

        // Verify response (should be echo)
        if response.len() != bytes_written {
            eprintln!(
                "Warning: Response size mismatch. Sent {} bytes, received {} bytes",
                bytes_written,
                response.len()
            );
        }

        latencies.push(elapsed);

        // Print progress
        if (i + 1) % 10 == 0 || i + 1 == count {
            println!("  Progress: {}/{}", i + 1, count);
        }

        // Small pause to avoid flooding
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Close connection
    net_engine.close(fd).await?;
    println!("Closed connection to echo server");

    Ok(latencies)
}
