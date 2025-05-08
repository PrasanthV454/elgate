//! Example of using the shared memory ring buffer.
//!
//! This example demonstrates creating, writing to, and reading from
//! the shared memory ring buffer.

use elgate_core::ring::{self, OperationKind, RingBufferOptions};
use std::path::PathBuf;
use std::time::Duration;
use std::thread;

fn main() {
    // Use a test-specific path to avoid conflicts
    let test_path = "/tmp/elgate_example_ring";
    
    // Create options with test path
    let options = RingBufferOptions {
        path: PathBuf::from(test_path),
        size: ring::DEFAULT_RING_SIZE,
        slot_size: ring::DEFAULT_SLOT_SIZE,
    };
    
    println!("Creating ring buffer at {}", test_path);
    
    // Create a producer thread
    let producer_options = options.clone();
    let producer_thread = thread::spawn(move || {
        let ring = ring::RingBuffer::create(producer_options).unwrap();
        
        println!("Producer: Ring buffer created with {} slots", ring.slot_count());
        
        // Write some data
        for i in 0..5 {
            let data = format!("Message {}", i).into_bytes();
            println!("Producer: Writing: {}", String::from_utf8_lossy(&data));
            
            let slot_idx = ring.write(OperationKind::DiskWrite, &data).unwrap();
            println!("Producer: Wrote to slot {}", slot_idx);
            
            // Wait a bit between writes
            thread::sleep(Duration::from_millis(500));
        }
        
        println!("Producer: Done writing");
    });
    
    // Wait a bit for the producer to start
    thread::sleep(Duration::from_millis(1000));
    
    // Create a consumer thread
    let consumer_options = options.clone();
    let consumer_thread = thread::spawn(move || {
        let ring = ring::RingBuffer::open(consumer_options).unwrap();
        
        println!("Consumer: Ring buffer opened with {} slots", ring.slot_count());
        
        // Read data
        let mut count = 0;
        while count < 5 {
            match ring.read().unwrap() {
                Some((kind, data)) => {
                    println!(
                        "Consumer: Read: {} (kind: {:?})",
                        String::from_utf8_lossy(&data),
                        kind
                    );
                    count += 1;
                }
                None => {
                    println!("Consumer: No data available, waiting...");
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
        
        println!("Consumer: Done reading");
    });
    
    // Wait for both threads to complete
    producer_thread.join().unwrap();
    consumer_thread.join().unwrap();
    
    println!("Example completed successfully!");
    
    // Clean up
    std::fs::remove_file(test_path).ok();
}
