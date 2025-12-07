// SPDX-License-Identifier: GPL-2.0

//! Typed error types for the bedrock-vm crate.

use std::fmt;
use std::io;

/// Error type for VM operations with context.
#[derive(Debug)]
pub enum VmError {
    /// The bedrock device was not found (module not loaded).
    DeviceNotFound,
    /// Permission denied when accessing the device.
    PermissionDenied,
    /// Cannot run or modify a parent VM while it has active children.
    ParentHasActiveChildren,
    /// Memory allocation failed.
    MemoryAllocationFailed { requested: usize },
    /// Invalid configuration provided.
    InvalidConfiguration { reason: String },
    /// An ioctl operation failed.
    Ioctl {
        operation: &'static str,
        source: io::Error,
    },
    /// Memory mapping failed.
    MmapFailed { source: io::Error },
    /// Parent VM not found for forking.
    ParentNotFound { id: u64 },
    /// Address out of bounds for guest memory.
    AddressOutOfBounds {
        gpa: u64,
        len: usize,
        memory_size: usize,
    },
    /// Generic I/O error.
    Io(io::Error),
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VmError::DeviceNotFound => {
                write!(f, "Bedrock device not found. Is the kernel module loaded?")
            }
            VmError::PermissionDenied => {
                write!(f, "Permission denied accessing bedrock device")
            }
            VmError::ParentHasActiveChildren => {
                write!(
                    f,
                    "Cannot modify parent VM while it has active forked children"
                )
            }
            VmError::MemoryAllocationFailed { requested } => {
                write!(f, "Memory allocation failed: requested {} bytes", requested)
            }
            VmError::InvalidConfiguration { reason } => {
                write!(f, "Invalid configuration: {}", reason)
            }
            VmError::Ioctl { operation, source } => {
                write!(f, "Ioctl {} failed: {}", operation, source)
            }
            VmError::MmapFailed { source } => {
                write!(f, "Memory mapping failed: {}", source)
            }
            VmError::ParentNotFound { id } => {
                write!(f, "Parent VM with ID {} not found", id)
            }
            VmError::AddressOutOfBounds {
                gpa,
                len,
                memory_size,
            } => {
                write!(
                    f,
                    "Address out of bounds: GPA {:#x} + {} exceeds memory size {}",
                    gpa, len, memory_size
                )
            }
            VmError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for VmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            VmError::Ioctl { source, .. } => Some(source),
            VmError::MmapFailed { source } => Some(source),
            VmError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for VmError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => VmError::DeviceNotFound,
            io::ErrorKind::PermissionDenied => VmError::PermissionDenied,
            _ => VmError::Io(e),
        }
    }
}

impl From<VmError> for io::Error {
    fn from(e: VmError) -> Self {
        match e {
            VmError::DeviceNotFound => io::Error::new(io::ErrorKind::NotFound, e.to_string()),
            VmError::PermissionDenied => {
                io::Error::new(io::ErrorKind::PermissionDenied, e.to_string())
            }
            VmError::ParentHasActiveChildren => {
                io::Error::new(io::ErrorKind::ResourceBusy, e.to_string())
            }
            VmError::MemoryAllocationFailed { .. } => {
                io::Error::new(io::ErrorKind::OutOfMemory, e.to_string())
            }
            VmError::InvalidConfiguration { .. } => {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            }
            VmError::Ioctl { source, .. } => source,
            VmError::MmapFailed { source } => source,
            VmError::ParentNotFound { .. } => {
                io::Error::new(io::ErrorKind::NotFound, e.to_string())
            }
            VmError::AddressOutOfBounds { .. } => {
                io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
            }
            VmError::Io(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = VmError::DeviceNotFound;
        assert!(err.to_string().contains("not found"));

        let err = VmError::MemoryAllocationFailed { requested: 1024 };
        assert!(err.to_string().contains("1024"));

        let err = VmError::ParentNotFound { id: 42 };
        assert!(err.to_string().contains("42"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "not found");
        let vm_err: VmError = io_err.into();
        assert!(matches!(vm_err, VmError::DeviceNotFound));

        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let vm_err: VmError = io_err.into();
        assert!(matches!(vm_err, VmError::PermissionDenied));
    }

    #[test]
    fn test_to_io_error() {
        let vm_err = VmError::DeviceNotFound;
        let io_err: io::Error = vm_err.into();
        assert_eq!(io_err.kind(), io::ErrorKind::NotFound);

        let vm_err = VmError::ParentHasActiveChildren;
        let io_err: io::Error = vm_err.into();
        assert_eq!(io_err.kind(), io::ErrorKind::ResourceBusy);
    }
}
