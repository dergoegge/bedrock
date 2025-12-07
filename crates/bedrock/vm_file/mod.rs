// SPDX-License-Identifier: GPL-2.0

//! VM file descriptor support for per-VM anonymous inodes.
//!
//! This module provides the file operations for bedrock-vm anonymous inodes,
//! which are created when userspace calls CREATE_ROOT_VM. Each VM gets its
//! own file descriptor, and the VM is released when the file descriptor is
//! closed.
//!
//! # Module Structure
//!
//! - [`structs`] - User ABI structures and ioctl definitions
//! - [`core`] - BedrockVmFile and BedrockForkedVmFile structs
//! - [`handlers`] - Shared trait-based ioctl handlers
//! - [`root`] - Root VM file operations
//! - [`forked`] - Forked VM file operations
//! - [`fd`] - FD creation functions

pub mod core;
pub mod fd;
pub mod forked;
pub mod handlers;
pub mod root;
pub mod structs;

// Re-export commonly used items
pub use core::{read_vm_file_type, BedrockForkedVmFile, BedrockVmFile, VmFileType};
pub use fd::{create_forked_vm_fd, create_vm_fd};
pub use structs::BedrockRegs;
