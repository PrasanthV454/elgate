//! Common utilities for examples

/// Checks if io_uring is fully supported in this environment
pub fn check_io_uring_full_support() -> bool {
    // Skip in CI environments by default
    if std::env::var("CI").is_ok() {
        println!("Running in CI environment, io_uring may not be fully supported");
        return false;
    }
    
    // Basic check for device existence
    let has_device = std::path::Path::new("/dev/io_uring").exists();
    if !has_device {
        println!("Missing /dev/io_uring device");
        return false;
    }
    
    // Check for kernel version (io_uring works best on 5.10+)
    if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
        let version = String::from_utf8_lossy(&output.stdout);
        let version_parts: Vec<&str> = version.split('.').collect();
        if version_parts.len() >= 2 {
            if let (Ok(major), Ok(minor)) = (version_parts[0].trim().parse::<u32>(), version_parts[1].trim().parse::<u32>()) {
                if major < 5 || (major == 5 && minor < 10) {
                    println!("Kernel version {}.{} may not fully support io_uring", major, minor);
                    return false;
                }
            }
        }
    }
    
    true
}
