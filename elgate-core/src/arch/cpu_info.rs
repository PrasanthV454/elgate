//! CPU information and topology detection
//!
//! This module handles detecting the number of CPU cores and NUMA nodes.

use std::fmt;

/// Information about a NUMA node including its ID and associated CPU cores
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumaNode {
    /// NUMA node ID
    pub id: usize,
    /// Logical CPU cores in this NUMA node
    pub cores: Vec<usize>,
}

impl NumaNode {
    /// Create a new NUMA node with the given ID and core list
    pub fn new(id: usize, cores: Vec<usize>) -> Self {
        Self { id, cores }
    }

    /// Get the number of cores in this NUMA node
    pub fn core_count(&self) -> usize {
        self.cores.len()
    }
}

/// CPU topology information including logical cores and NUMA nodes
#[derive(Clone)]
pub struct CpuInfo {
    /// Total number of logical CPU cores
    logical_cores: usize,
    /// Total number of physical CPU cores (if available)
    physical_cores: Option<usize>,
    /// NUMA nodes detected on the system
    numa_nodes: Vec<NumaNode>,
    /// Is NUMA topology valid and usable?
    numa_available: bool,
}

impl fmt::Debug for CpuInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpuInfo")
            .field("logical_cores", &self.logical_cores)
            .field("physical_cores", &self.physical_cores)
            .field("numa_nodes", &self.numa_nodes.len())
            .field("numa_available", &self.numa_available)
            .finish()
    }
}

impl CpuInfo {
    /// Detect CPU topology on the current system
    pub fn detect() -> Self {
        // Get logical CPU count
        let logical_cores = num_cpus::get();
        
        // Try to get physical core count (may not be available on all platforms)
        let physical_cores = match num_cpus::get_physical() {
            count if count > 0 => Some(count),
            _ => None,
        };
        
        // Detect NUMA topology
        let (numa_nodes, numa_available) = Self::detect_numa_nodes(logical_cores);
        
        Self {
            logical_cores,
            physical_cores,
            numa_nodes,
            numa_available,
        }
    }
    
    /// Get the total number of logical CPU cores
    pub fn logical_cores(&self) -> usize {
        self.logical_cores
    }
    
    /// Get the total number of physical CPU cores (if available)
    pub fn physical_cores(&self) -> Option<usize> {
        self.physical_cores
    }
    
    /// Get a list of all NUMA nodes
    pub fn numa_nodes(&self) -> &[NumaNode] {
        &self.numa_nodes
    }
    
    /// Check if NUMA topology is available
    pub fn is_numa_available(&self) -> bool {
        self.numa_available
    }
    
    /// Get CPU cores for a specific worker index with NUMA awareness
    pub fn get_core_for_worker(&self, worker_idx: usize) -> usize {
        if !self.numa_available || self.numa_nodes.is_empty() {
            // Without NUMA, just distribute workers evenly across all cores
            return worker_idx % self.logical_cores;
        }
        
        // Distribute workers evenly across NUMA nodes
        let mut total_cores = 0;
        for node in &self.numa_nodes {
            total_cores += node.core_count();
            if worker_idx < total_cores {
                // Worker belongs to this NUMA node
                let local_idx = worker_idx - (total_cores - node.core_count());
                return node.cores[local_idx % node.core_count()];
            }
        }

        // Fallback if something went wrong
        worker_idx % self.logical_cores
    }

    /// Detect NUMA nodes on the system
    fn detect_numa_nodes(logical_cores: usize) -> (Vec<NumaNode>, bool) {
        // Try to detect NUMA topology
        #[cfg(target_os = "linux")]
        {
            // On Linux, we try to read NUMA info from sysfs
            match Self::detect_numa_linux() {
                Ok(nodes) if !nodes.is_empty() => return (nodes, true),
                _ => {}
            }
        }
        
        // Fallback: create a single NUMA node with all cores
        let fallback_node = NumaNode::new(0, (0..logical_cores).collect());
        (vec![fallback_node], false)
    }
    
    // Linux-specific NUMA detection
    #[cfg(target_os = "linux")]
    fn detect_numa_linux() -> Result<Vec<NumaNode>, std::io::Error> {
        use std::fs;
        use std::path::Path;
        
        let numa_path = Path::new("/sys/devices/system/node");
        if !numa_path.exists() {
            return Ok(vec![]);
        }
        
        let mut numa_nodes = Vec::new();
        
        // Read node directories (node0, node1, etc.)
        for entry in fs::read_dir(numa_path)? {
            let entry = entry?;
            let path = entry.path();
            
            // Check if this is a node directory
            let file_name = path.file_name().unwrap().to_string_lossy();
            if !file_name.starts_with("node") {
                continue;
            }
            
            // Parse node ID
            let node_id = match file_name.trim_start_matches("node").parse::<usize>() {
                Ok(id) => id,
                Err(_) => continue,
            };
            
            // Read CPU list
            let cpulist_path = path.join("cpulist");
            if !cpulist_path.exists() {
                continue;
            }
            
            let cpulist = fs::read_to_string(cpulist_path)?;
            let cores = Self::parse_cpu_list(&cpulist);
            
            numa_nodes.push(NumaNode::new(node_id, cores));
        }
        
        Ok(numa_nodes)
    }
    
    // Non-Linux platforms: stub implementation
    #[cfg(not(target_os = "linux"))]
    fn detect_numa_linux() -> Result<Vec<NumaNode>, std::io::Error> {
        Ok(vec![])
    }
    
    /// Parse a CPU list string like "0-2,4,6-8" into a vector of core IDs
    fn parse_cpu_list(cpulist: &str) -> Vec<usize> {
        let mut cores = Vec::new();
        
        for part in cpulist.split(',') {
            if part.contains('-') {
                // Range like "0-3"
                let range: Vec<&str> = part.split('-').collect();
                if range.len() == 2 {
                    if let (Ok(start), Ok(end)) = (range[0].trim().parse::<usize>(), range[1].trim().parse::<usize>()) {
                        cores.extend(start..=end);
                    }
                }
            } else {
                // Single core like "4"
                if let Ok(core) = part.trim().parse::<usize>() {
                    cores.push(core);
                }
            }
        }
        
        cores
    }
    
    /// Create a mock CpuInfo for testing
    #[cfg(test)]
    pub fn mock(logical_cores: usize, numa_nodes_count: usize) -> Self {
        let mut numa_nodes = Vec::new();
        
        if numa_nodes_count > 0 {
            // Distribute cores evenly among NUMA nodes
            let cores_per_node = logical_cores / numa_nodes_count;
            let mut remaining_cores = logical_cores % numa_nodes_count;
            
            let mut core_idx = 0;
            for node_id in 0..numa_nodes_count {
                let extra_core = if remaining_cores > 0 { 1 } else { 0 };
                remaining_cores = remaining_cores.saturating_sub(1);
                
                let node_core_count = cores_per_node + extra_core;
                let mut node_cores = Vec::with_capacity(node_core_count);
                
                for _ in 0..node_core_count {
                    node_cores.push(core_idx);
                    core_idx += 1;
                }
                
                numa_nodes.push(NumaNode::new(node_id, node_cores));
            }
        } else {
            // Single NUMA node with all cores
            numa_nodes.push(NumaNode::new(0, (0..logical_cores).collect()));
        }
        
        Self {
            logical_cores,
            physical_cores: Some(logical_cores / 2),
            numa_nodes,
            numa_available: numa_nodes_count > 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cpu_info_detect() {
        let info = CpuInfo::detect();
        println!("Detected CPU info: {:?}", info);
        
        // At minimum, we should have at least one core
        assert!(info.logical_cores() > 0);
        
        // Should have at least one NUMA node (even if it's a fallback)
        assert!(!info.numa_nodes().is_empty());
    }
    
    #[test]
    fn test_parse_cpu_list() {
        assert_eq!(CpuInfo::parse_cpu_list("0-2,4,6-8"), vec![0, 1, 2, 4, 6, 7, 8]);
        assert_eq!(CpuInfo::parse_cpu_list("0"), vec![0]);
        assert_eq!(CpuInfo::parse_cpu_list("0-3"), vec![0, 1, 2, 3]);
        assert_eq!(CpuInfo::parse_cpu_list(""), Vec::<usize>::new());
    }
    
    #[test]
    fn test_mock_cpu_info() {
        // Test single NUMA node
        let info = CpuInfo::mock(8, 1);
        assert_eq!(info.logical_cores(), 8);
        assert_eq!(info.numa_nodes().len(), 1);
        assert_eq!(info.numa_nodes()[0].cores.len(), 8);
        
        // Test multiple NUMA nodes
        let info = CpuInfo::mock(8, 2);
        assert_eq!(info.logical_cores(), 8);
        assert_eq!(info.numa_nodes().len(), 2);
        assert_eq!(info.numa_nodes()[0].cores.len(), 4);
        assert_eq!(info.numa_nodes()[1].cores.len(), 4);
        
        // Test uneven distribution
        let info = CpuInfo::mock(7, 2);
        assert_eq!(info.logical_cores(), 7);
        assert_eq!(info.numa_nodes().len(), 2);
        assert_eq!(info.numa_nodes()[0].cores.len(), 4);
        assert_eq!(info.numa_nodes()[1].cores.len(), 3);
    }
    
    #[test]
    fn test_get_core_for_worker() {
        // Test with 8 cores, 2 NUMA nodes
        let info = CpuInfo::mock(8, 2);
        
        // First 4 workers should get cores from the first NUMA node
        assert_eq!(info.get_core_for_worker(0), 0);
        assert_eq!(info.get_core_for_worker(1), 1);
        assert_eq!(info.get_core_for_worker(2), 2);
        assert_eq!(info.get_core_for_worker(3), 3);
        
        // Next 4 workers should get cores from the second NUMA node
        assert_eq!(info.get_core_for_worker(4), 4);
        assert_eq!(info.get_core_for_worker(5), 5);
        assert_eq!(info.get_core_for_worker(6), 6);
        assert_eq!(info.get_core_for_worker(7), 7);
        
        // Additional workers wrap around within their NUMA node
        assert_eq!(info.get_core_for_worker(8), 0);
        assert_eq!(info.get_core_for_worker(9), 1);
    }
}
