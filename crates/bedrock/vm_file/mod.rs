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

pub(crate) mod core;
pub(crate) mod fd;
pub(crate) mod forked;
pub(crate) mod handlers;
pub(crate) mod root;
pub(crate) mod structs;

// Re-export commonly used items
pub(crate) use core::{read_vm_file_type, BedrockForkedVmFile, BedrockVmFile, VmFileType};
pub(crate) use fd::{create_forked_vm_fd, create_vm_fd};
