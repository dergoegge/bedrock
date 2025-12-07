// SPDX-License-Identifier: GPL-2.0

//! Ioctl encoding and constants for bedrock device.

use std::mem::size_of;

use super::config::{LogConfig, SingleStepConfig};
use super::exit::VmExit;
use super::serial::SerialInput;
use super::stats::ExitStats;
use crate::rdrand::RdrandConfig;
use crate::Regs;

/// Ioctl magic number ('B' for Bedrock).
const BEDROCK_IOC_MAGIC: u8 = b'B';

// Ioctl direction bits
pub(super) const IOC_WRITE: u64 = 1;
pub(super) const IOC_READ: u64 = 2;

// Ioctl encoding shifts
const IOC_NRSHIFT: u64 = 0;
const IOC_TYPESHIFT: u64 = 8;
const IOC_SIZESHIFT: u64 = 16;
const IOC_DIRSHIFT: u64 = 30;

/// Encode an ioctl number for reading data (_IOR).
const fn ioctl_ior(ty: u8, nr: u8, size: usize) -> u64 {
    ((IOC_READ) << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

/// Encode an ioctl number for writing data (_IOW).
const fn ioctl_iow(ty: u8, nr: u8, size: usize) -> u64 {
    ((IOC_WRITE) << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)
}

// Device ioctls (on /dev/bedrock)
// _IOW('B', 0, u64) - takes memory size as argument
pub(crate) const BEDROCK_CREATE_ROOT_VM: u64 = ioctl_iow(BEDROCK_IOC_MAGIC, 0, size_of::<u64>());

// VM ioctls (on VM file descriptor)
pub(crate) const BEDROCK_VM_GET_REGS: u64 = ioctl_ior(BEDROCK_IOC_MAGIC, 1, size_of::<Regs>());
pub(crate) const BEDROCK_VM_SET_REGS: u64 = ioctl_iow(BEDROCK_IOC_MAGIC, 2, size_of::<Regs>());
pub(crate) const BEDROCK_VM_RUN: u64 = ioctl_ior(BEDROCK_IOC_MAGIC, 3, size_of::<VmExit>());
pub(crate) const BEDROCK_VM_SET_INPUT: u64 =
    ioctl_iow(BEDROCK_IOC_MAGIC, 4, size_of::<SerialInput>());
pub(crate) const BEDROCK_VM_SET_RDRAND_CONFIG: u64 =
    ioctl_iow(BEDROCK_IOC_MAGIC, 5, size_of::<RdrandConfig>());
pub(crate) const BEDROCK_VM_SET_RDRAND_VALUE: u64 =
    ioctl_iow(BEDROCK_IOC_MAGIC, 6, size_of::<u64>());
pub(crate) const BEDROCK_VM_SET_LOG_CONFIG: u64 =
    ioctl_iow(BEDROCK_IOC_MAGIC, 7, size_of::<LogConfig>());
pub(crate) const BEDROCK_VM_SET_SINGLE_STEP: u64 =
    ioctl_iow(BEDROCK_IOC_MAGIC, 8, size_of::<SingleStepConfig>());
pub(crate) const BEDROCK_VM_GET_EXIT_STATS: u64 =
    ioctl_ior(BEDROCK_IOC_MAGIC, 9, size_of::<ExitStats>());
pub(crate) const BEDROCK_VM_SET_STOP_TSC: u64 = ioctl_iow(BEDROCK_IOC_MAGIC, 10, size_of::<u64>());
pub(crate) const BEDROCK_VM_GET_VM_ID: u64 = ioctl_ior(BEDROCK_IOC_MAGIC, 11, size_of::<u64>());

// Device ioctls (on /dev/bedrock)
// _IOW('B', 1, u64) - takes parent VM ID as argument
pub(crate) const BEDROCK_CREATE_FORKED_VM: u64 = ioctl_iow(BEDROCK_IOC_MAGIC, 1, size_of::<u64>());

/// Maximum number of feedback buffers per VM.
pub const MAX_FEEDBACK_BUFFERS: usize = 16;

/// Request structure for getting feedback buffer info.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct FeedbackBufferInfoRequest {
    /// Buffer index to query (0-15).
    pub index: u32,
    /// Reserved for alignment.
    pub _reserved: u32,
}

// _IOR('B', 12, FeedbackBufferInfoRequest) - get feedback buffer registration info
pub(crate) const BEDROCK_VM_GET_FEEDBACK_BUFFER_INFO: u64 = ioctl_ior(
    BEDROCK_IOC_MAGIC,
    12,
    size_of::<FeedbackBufferInfoRequest>(),
);

/// Feedback buffer info returned from kernel.
///
/// This structure describes a feedback buffer registered by the guest
/// via the HYPERCALL_REGISTER_FEEDBACK_BUFFER hypercall.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct FeedbackBufferInfo {
    /// Original guest virtual address.
    pub gva: u64,
    /// Size in bytes.
    pub size: u64,
    /// Number of pages.
    pub num_pages: u64,
    /// Whether a feedback buffer is registered (0 = no, 1 = yes).
    pub registered: u32,
    /// Buffer index (0-15).
    pub index: u32,
    /// Reserved for alignment.
    pub _reserved: u32,
}
