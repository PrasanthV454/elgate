//! NUMA-Aware Ring Buffer example for Elgate.
//!
//! This example demonstrates how to use the NUMA-aware ring buffer
//! for optimized inter-process communication on NUMA systems.

use elgate_core::arch::cpu_info::CpuInfo;
use elgate_core::ring::{OperationKind, RingBuffer, RingBufferOptions};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    // Detect CPU information
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
        println!("This example will still run, but won't show NUMA benefits.");
    }

    // Create paths for regular and NUMA-aware ring buffers
    let regular_path = "/tmp/elgate_example_regular_ring";
    let numa_path = "/tmp/elgate_example_numa_ring";

    // Clean up from previous runs
    let _ = fs::remove_file(regular_path);
    let _ = fs::remove_file(numa_path);

    // Create ring buffer options
    let regular_options = RingBufferOptions {
        path: PathBuf::from(regular_path),
        size: 1024 * 1024, // 1 MiB
        slot_size: 4096,   // 4 KiB
    };

    let numa_options = RingBufferOptions {
        path: PathBuf::from(numa_path),
        size: 1024 * 1024, // 1 MiB
        slot_size: 4096,   // 4 KiB
    };

    // Create regular ring buffer
    println!("\nCreating regular ring buffer...");
    let regular_ring = Arc::new(RingBuffer::create(regular_options.clone())?);

    // Create NUMA-aware ring buffer
    println!("Creating NUMA-aware ring buffer...");
    let numa_node = 0; // Use the first NUMA node
    let numa_ring = Arc::new(RingBuffer::create_numa_aware(
        numa_options.clone(),
        numa_node,
    )?);

    println!("Both ring buffers created successfully.");

    // Compare performance between regular and NUMA-aware ring buffers
    println!("\nComparing performance (writing and reading 10,000 messages)...");

    // Test regular ring buffer
    println!("Testing regular ring buffer...");
    let regular_start = Instant::now();

    let data = vec![42u8; 1024]; // 1 KiB of data
    let iterations = 10_000;

    // Write data
    for _i in 0..iterations {
        regular_ring.write(OperationKind::DiskWrite, &data)?;
    }

    // Read data
    for _ in 0..iterations {
        let result = regular_ring.read()?;
        assert!(result.is_some());
    }

    let regular_duration = regular_start.elapsed();
    let regular_throughput = (iterations as f64) / regular_duration.as_secs_f64();

    println!(
        "Regular ring buffer: {} ops in {:?}",
        iterations, regular_duration
    );
    println!("Regular throughput: {:.2} ops/sec", regular_throughput);

    // Test NUMA-aware ring buffer
    println!("\nTesting NUMA-aware ring buffer...");
    let numa_start = Instant::now();

    // Write data
    for _i in 0..iterations {
        numa_ring.write(OperationKind::DiskWrite, &data)?;
    }

    // Read data
    for _ in 0..iterations {
        let result = numa_ring.read()?;
        assert!(result.is_some());
    }

    let numa_duration = numa_start.elapsed();
    let numa_throughput = (iterations as f64) / numa_duration.as_secs_f64();

    println!(
        "NUMA-aware ring buffer: {} ops in {:?}",
        iterations, numa_duration
    );
    println!("NUMA throughput: {:.2} ops/sec", numa_throughput);

    // Calculate improvement
    let improvement = (numa_throughput / regular_throughput - 1.0) * 100.0;
    println!("\nPerformance difference: {:.2}%", improvement);

    if improvement > 0.0 {
        println!("NUMA-aware ring buffer is faster");
    } else {
        println!("Regular ring buffer is faster (or no significant difference)");
        println!("This can happen on systems without real NUMA topology");
    }

    // Multi-producer test
    println!("\nTesting multi-producer scenario...");

    // Clean up and create new ring buffers
    let _ = fs::remove_file(regular_path);
    let _ = fs::remove_file(numa_path);

    let regular_ring = Arc::new(RingBuffer::create(regular_options.clone())?);
    let numa_ring = Arc::new(RingBuffer::create_numa_aware(
        numa_options.clone(),
        numa_node,
    )?);

    // Number of producer threads
    let num_producers = 4;
    let iterations_per_producer = 2_500; // 10,000 total

    // Regular ring buffer test
    println!(
        "Testing regular ring buffer with {} producers...",
        num_producers
    );
    let regular_start = Instant::now();

    // Create producer threads
    let mut regular_handles = Vec::new();
    for producer_id in 0..num_producers {
        let ring = regular_ring.clone();
        let data = vec![producer_id as u8; 1024]; // 1 KiB of data

        let handle = thread::spawn(move || {
            for _i in 0..iterations_per_producer {
                ring.write(OperationKind::DiskWrite, &data).unwrap();
            }
        });

        regular_handles.push(handle);
    }

    // Wait for producers to finish
    for handle in regular_handles {
        handle.join().unwrap();
    }

    // Read all data
    for _ in 0..(iterations_per_producer * num_producers) {
        let result = regular_ring.read()?;
        assert!(result.is_some());
    }

    let regular_mp_duration = regular_start.elapsed();
    let regular_mp_throughput =
        ((iterations_per_producer * num_producers) as f64) / regular_mp_duration.as_secs_f64();

    println!(
        "Regular multi-producer: {} ops in {:?}",
        iterations_per_producer * num_producers,
        regular_mp_duration
    );
    println!(
        "Regular multi-producer throughput: {:.2} ops/sec",
        regular_mp_throughput
    );

    // NUMA-aware ring buffer test
    println!(
        "\nTesting NUMA-aware ring buffer with {} producers...",
        num_producers
    );
    let numa_start = Instant::now();

    // Create producer threads
    let mut numa_handles = Vec::new();
    for producer_id in 0..num_producers {
        let ring = numa_ring.clone();
        let data = vec![producer_id as u8; 1024]; // 1 KiB of data

        let handle = thread::spawn(move || {
            for _i in 0..iterations_per_producer {
                ring.write(OperationKind::DiskWrite, &data).unwrap();
            }
        });

        numa_handles.push(handle);
    }

    // Wait for producers to finish
    for handle in numa_handles {
        handle.join().unwrap();
    }

    // Read all data
    for _ in 0..(iterations_per_producer * num_producers) {
        let result = numa_ring.read()?;
        assert!(result.is_some());
    }

    let numa_mp_duration = numa_start.elapsed();
    let numa_mp_throughput =
        ((iterations_per_producer * num_producers) as f64) / numa_mp_duration.as_secs_f64();

    println!(
        "NUMA-aware multi-producer: {} ops in {:?}",
        iterations_per_producer * num_producers,
        numa_mp_duration
    );
    println!(
        "NUMA-aware multi-producer throughput: {:.2} ops/sec",
        numa_mp_throughput
    );

    // Calculate multi-producer improvement
    let mp_improvement = (numa_mp_throughput / regular_mp_throughput - 1.0) * 100.0;
    println!(
        "\nMulti-producer performance difference: {:.2}%",
        mp_improvement
    );

    if mp_improvement > 0.0 {
        println!("NUMA-aware ring buffer is faster for multi-producer scenario");
    } else {
        println!("Regular ring buffer is faster for multi-producer scenario (or no significant difference)");
    }

    // Clean up
    let _ = fs::remove_file(regular_path);
    let _ = fs::remove_file(numa_path);

    println!("\nNUMA-aware ring buffer example completed successfully!");
    Ok(())
}
