# Changelog

All notable changes to the Elgate project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Refactored IO engines to use tokio-uring directly**:
  - Eliminated worker thread approach to fix "invalid context" errors
  - Uses tokio-uring runtime for all IO operations  
  - Simplified architecture with direct file handling
  - Lower latency for IO operations by removing thread communication overhead
  - Fixed CI pipeline to properly test io_uring capabilities

### Fixed
- Resolved "Attempted to access driver in invalid context" errors in tokio-uring operations
- Fixed examples to work correctly in CI environments
