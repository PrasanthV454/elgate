use elgate_core::arch;

fn main() {
    // Detect CPU topology
    let cpu_info = arch::detect_cpu_topology();
    println!("Detected {} logical cores", cpu_info.logical_cores());

    if let Some(physical) = cpu_info.physical_cores() {
        println!("Detected {} physical cores", physical);
    } else {
        println!("Physical core count not available");
    }

    println!("Detected {} NUMA nodes", cpu_info.numa_nodes().len());
    println!("NUMA available: {}", cpu_info.is_numa_available());

    // Print details of each NUMA node
    for (_i, node) in cpu_info.numa_nodes().iter().enumerate() {
        println!(
            "NUMA Node {}: {} cores ({:?})",
            node.id,
            node.core_count(),
            node.cores
        );
    }

    // Select runtime mode
    let mode = arch::select_runtime_mode(&cpu_info);
    println!("\nSelected runtime mode: {:?}", mode);
    println!("Mode description: {}", mode.description());
    println!("Worker count: {}", mode.worker_count());
    println!("NUMA aware: {}", mode.is_numa_aware());
    println!("Supports pinning: {}", mode.supports_pinning());
}
