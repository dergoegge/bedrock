// =============================================================================
// XSAVE State Management
// =============================================================================

/// XCR0 register bits for XSAVE feature control.
/// See Intel SDM Vol 3A, Section 2.6.
pub mod xcr0 {
    /// x87 FPU/MMX state (must always be 1).
    const X87: u64 = 1 << 0;
    /// SSE state (MXCSR and XMM0-XMM15).
    const SSE: u64 = 1 << 1;
    /// AVX state (upper halves of YMM0-YMM15).
    const AVX: u64 = 1 << 2;

    /// Standard XCR0 value for SSE + AVX support.
    pub const SSE_AVX: u64 = X87 | SSE | AVX;
}
