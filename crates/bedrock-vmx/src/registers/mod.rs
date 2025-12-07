// =============================================================================
// Register Abstractions for x86-64 Virtualization
// =============================================================================

mod cr;
mod descriptor;
mod dr;
mod gpr;
mod msr_defs;
mod segment;
mod syscall;
mod xcr;

// Re-export all public types

// General-purpose registers
pub use gpr::GeneralPurposeRegisters;

// Control registers
pub use cr::{ControlRegisters, Cr0, Cr2, Cr3, Cr4, Cr8, CrAccess, CrError, CrResult};

// Debug registers
pub use dr::DebugRegisters;

// Segment registers
pub use segment::{SegmentAccessRights, SegmentRegister, SegmentRegisters, SegmentSelector};

// Descriptor table registers
pub use descriptor::{DescriptorTableAccess, DescriptorTableRegisters, Gdtr, Idtr};

// MSRs
pub use msr_defs::{msr, MsrAccess, MsrError, MsrResult};

// SYSCALL-related MSRs
pub use syscall::{Cstar, Efer, ExtendedControlRegisters, Fmask, Lstar, MiscEnable, Star};

// XSAVE state
pub use xcr::xcr0;
