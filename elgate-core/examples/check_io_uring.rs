//! Checks if io_uring is fully supported and usable
//! This example performs various checks to ensure io_uring works correctly

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;

fn main() {
    println!("Checking io_uring support and permissions...");

    // Check kernel version
    if let Ok(output) = Command::new("uname").arg("-r").output() {
        let version = String::from_utf8_lossy(&output.stdout);
        println!("Kernel version: {}", version.trim());

        // io_uring was introduced in 5.1, but many features came in 5.6+
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() >= 2 {
            if let (Ok(major), Ok(minor)) = (
                parts[0].trim().parse::<u32>(),
                parts[1].trim().parse::<u32>(),
            ) {
                if major < 5 || (major == 5 && minor < 6) {
                    println!("⚠️ WARNING: Kernel version may not fully support io_uring (best with 5.6+)");
                }
            }
        }
    }

    // Check if /dev/io_uring exists
    println!("\nChecking for /dev/io_uring:");
    if std::path::Path::new("/dev/io_uring").exists() {
        println!("✅ /dev/io_uring exists");

        // Check permissions on /dev/io_uring
        if let Ok(metadata) = std::fs::metadata("/dev/io_uring") {
            let permissions = metadata.permissions();
            let readonly = permissions.readonly();
            println!(
                "   Permissions: {}",
                if readonly {
                    "read-only ⚠️"
                } else {
                    "read-write ✅"
                }
            );
        }
    } else {
        println!("❌ /dev/io_uring does not exist");
    }

    // Check memory lock limits
    println!("\nChecking memory lock limits (ulimit -l):");
    if let Ok(output) = Command::new("sh").arg("-c").arg("ulimit -l").output() {
        let limit = String::from_utf8_lossy(&output.stdout).trim().to_string();
        println!("Memory lock limit: {} KB", limit);

        if let Ok(limit_val) = limit.parse::<u64>() {
            if limit_val < 1024 {
                println!("⚠️ WARNING: Low memory lock limit may cause io_uring to fail");
                println!("   Recommended: at least 1024 KB");
            } else {
                println!("✅ Memory lock limit is sufficient");
            }
        }
    }

    // Try to do a basic io_uring operation
    println!("\nAttempting to use io_uring:");
    let test_path = "/tmp/io_uring_test_write";
    match OpenOptions::new()
        .write(true)
        .create(true)
        .custom_flags(libc::O_DIRECT)
        .open(test_path)
    {
        Ok(_) => {
            println!("✅ Successfully opened file with O_DIRECT flag");
            std::fs::remove_file(test_path).ok();
        }
        Err(e) => {
            println!("❌ Failed to open file with O_DIRECT: {}", e);
        }
    }

    // Try to use tokio-uring in a minimal example
    println!("\nAttempting to initialize tokio-uring:");
    let result = std::thread::spawn(|| {
        tokio_uring::start(async {
            println!("✅ Successfully initialized tokio-uring runtime");
            // Just a minimal test
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        });
    })
    .join();

    match result {
        Ok(_) => println!("✅ tokio-uring test completed successfully"),
        Err(e) => println!("❌ tokio-uring test failed: {:?}", e),
    }

    // Check if running in CI
    if std::env::var("CI").is_ok() {
        println!("\n⚠️ Running in CI environment - io_uring may have limited functionality");
    }

    // Additional check for io_uring system call support
    #[cfg(target_os = "linux")]
    unsafe {
        println!("\nChecking io_uring syscall availability:");
        let setup_res = libc::syscall(
            libc::SYS_io_uring_setup,
            4,
            std::ptr::null::<libc::c_void>(),
        );
        if setup_res == -1 {
            let err = std::io::Error::last_os_error();
            println!("io_uring_setup syscall result: {}", err);
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                println!("❌ Permission denied for io_uring_setup syscall");
            } else if err.kind() == std::io::ErrorKind::NotFound {
                println!("❌ io_uring_setup syscall not found");
            } else {
                // EINVAL is normal since we passed null params
                println!("✅ io_uring_setup syscall available");
            }
        } else {
            println!("Unexpected success with null params? result: {}", setup_res);
        }
    }

    println!("\nSummary:");
    println!("If you're seeing failures, consider:");
    println!("1. Updating your kernel to 5.6+ for better io_uring support");
    println!("2. Increasing memory lock limits: sudo prlimit --memlock=unlimited --pid $$");
    println!("3. Running with elevated permissions or adding capabilities");
    println!("4. Using a non-containerized environment for testing");
}
