//! Common utilities for examples

/// Checks if io_uring is fully supported in this environment
pub fn check_io_uring_full_support() -> bool {
    // Perform a real io_uring operation
    // Create a temporary file to test with
    let test_file = "/tmp/elgate_iouring_test";
    let test_data = b"This is a test";

    // First write test file using regular IO
    if let Err(e) = std::fs::write(test_file, test_data) {
        println!("Failed to create test file: {}", e);
        return false;
    }

    // Try to use tokio-uring to read the file
    println!("Testing io_uring with a real file read operation...");
    let result = std::thread::spawn(move || -> bool {
        use tokio_uring::fs::File;

        tokio_uring::start(async {
            // Try to open and read the file
            match File::open(test_file).await {
                Ok(file) => {
                    // Allocate a buffer for reading
                    let mut buf = vec![0u8; 100];

                    // Read from the file - note that tokio-uring takes ownership of the buffer
                    let (res, buf) = file.read_at(buf, 0).await;

                    match res {
                        Ok(n) => {
                            println!("✅ Successfully read {} bytes using io_uring", n);
                            let content = String::from_utf8_lossy(&buf[0..n]);
                            println!("   Content: \"{}\"", content);

                            // Verify the content
                            let expected = String::from_utf8_lossy(test_data);
                            if content == expected {
                                println!("✅ Content matches, io_uring is fully functional");
                                return true;
                            } else {
                                println!("❌ Content mismatch, io_uring may have issues");
                                return false;
                            }
                        }
                        Err(e) => {
                            println!("❌ io_uring read failed: {}", e);
                            return false;
                        }
                    }
                }
                Err(e) => {
                    println!("❌ io_uring file open failed: {}", e);
                    return false;
                }
            }
        })
    })
    .join();

    // Clean up the test file
    let _ = std::fs::remove_file(test_file);

    // Check result
    match result {
        Ok(success) => success,
        Err(e) => {
            println!("❌ io_uring test panicked: {:?}", e);
            return false;
        }
    }
}
