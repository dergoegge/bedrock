# Bedrock Hypervisor

Rust-based x86-64 hypervisor running as a Linux kernel module, purpose-built
for deterministic software testing. All core logic is implemented against traits
that abstract hardware and kernel features, enabling userspace testing without
VMX, kernel, or root.

## Determinism

Guest execution must be fully deterministic — identical inputs must produce
identical outputs across runs. All sources of non-determinism visible to the
guest are trapped and emulated:

- **TSC**: RDTSC/RDTSCP exit to the hypervisor; an emulated TSC advances at a
  fixed frequency (2,995,200,000 Hz) based on exit counts, not wall-clock time.
- **RDRAND/RDSEED**: Trapped and served from controlled randomness modes.
- **Interrupts**: Timer interrupts are injected at deterministic points via
  emulated APIC timers driven by the emulated TSC.
- **CPUID**: Fully emulated to mask host-specific feature differences.
- **Devices**: All I/O is emulated (serial, RTC, APIC) with no passthrough.

Any change that leaks host state or timing into the guest breaks determinism.
When adding or modifying exit handlers or device emulation, ensure no
host-dependent values reach guest-visible state.

## Dual-Build System

Each crate has both `lib.rs` (Cargo builds/tests) and `mod.rs` (kernel module
builds). The `cargo` feature flag controls which is used:

- `#[cfg(feature = "cargo")]` — Cargo builds (default). Logging compiles to no-ops.
- `#[cfg(not(feature = "cargo"))]` — Kernel builds. Logging uses kernel `pr_*` macros.

This is most commonly needed for imports. The `bedrock-vmx` prelude centralizes
conditional imports for common types:

```rust
#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;
```

Add commonly-imported types to `prelude.rs` rather than duplicating `#[cfg]`
blocks. Symlinks from `crates/bedrock/` to other crates' `src/` directories
enable the kernel build to include them as submodules.

## Conventions

- Use `log_info!`, `log_err!`, `log_warn!`, `log_debug!` from `bedrock-log`
  instead of kernel `pr_*!` macros. In `crates/bedrock/`, these are available
  via `#[macro_use]` with no explicit import needed.
- Kernel stack is only 8KB. Large structures (`VmState`, `DeviceStates`,
  `ExitStats`) must be boxed. Use `scripts/check_stack.py` to verify.
- Test mocks are in `bedrock-vmx/test_mocks.rs`, gated behind the `test-utils`
  feature.

## Building

```bash
just test       # Run cargo tests
just remote     # Sync and build kernel module on remote host
just fmt        # Format code
```

Configure `REMOTE_HOST` and `REMOTE_DIR` in `.env` (see `.env.example`).

**Do not build the kernel module locally with Claude** — it requires a
configured Linux kernel build environment. Use `just remote` instead.

**Always verify both**: `just test` (cargo tests pass) and `just remote`
(kernel module compiles).

## Skills

- **`/sdm`**: Intel SDM — VMX, VMCS, EPT, MSRs, control registers, instruction semantics.
- **`/linux`**: Linux kernel (6.18) — KVM/VMX, memory management, drivers, syscalls.
- **`/bhyve`**: FreeBSD bhyve — compare virtualization approaches.
- **`/determ-analysis`**: Analyze determinism test results on the remote machine.
