# Nix Build System

Nix flake for building and testing the bedrock hypervisor with nested KVM.

## Quick Start

```bash
nix run .#vm            # Boot interactive dev VM (SSH on port 2222)
nix run .#test          # Run integration tests in NixOS VM
nix run .#test-native   # Run tests directly on host (faster, no nested virt)
```

## Packages

| Package | Description |
|---------|-------------|
| `kernel` | Linux 6.18 with `CONFIG_RUST=y` (no KVM) |
| `bedrockModule` | `bedrock.ko` kernel module |
| `guestKernel` | Linux 6.18 with determinism patches (TLB flush) + `vmlinux` |
| `guestInitrd` | Trivial initramfs (boots, VMCALL shutdown) |
| `podmanInitrd` | Podman initramfs (Bitcoin Core workload) |
| `bedrock-cli` | CLI for loading and running guest VMs |
| `bedrock-determinism` | Determinism checker (multi-run comparison) |

Build any with `nix build .#<name>`.

## Host Requirements

### Nix Configuration (`/etc/nix/nix.conf`)

```
experimental-features = nix-command flakes
sandbox = relaxed
extra-sandbox-paths = /dev/kvm
```

- **`nix-command flakes`**: Required for `nix build`, `nix run`, etc.
- **`sandbox = relaxed`**: The podman initrd is a fixed-output derivation that
  needs network access for `apt-get`. With `sandbox = true`, all network is
  blocked. `relaxed` allows FODs to access the network while keeping other
  builds sandboxed.
- **`extra-sandbox-paths = /dev/kvm`**: Exposes KVM to the build sandbox so
  `nix run .#test` can launch the NixOS test VM with hardware acceleration.

Restart the daemon after changes: `systemctl restart nix-daemon`

### For `nix run .#test` (NixOS VM tests)

- KVM-capable host with nested VMX support
- KVM modules loaded (`kvm`, `kvm_intel`) -- bedrock must NOT be loaded
  (it owns VMX exclusively; unload with `rmmod bedrock` first)

### For `nix run .#test-native` (bare-metal tests)

- Host kernel 6.18 with bedrock module loaded
- `/dev/bedrock` device present
- KVM must NOT be loaded (bedrock owns VMX)

### For `nix run .#vm` (interactive dev VM)

- Same as `nix run .#test` requirements
- SSH into the VM: `ssh -p 2222 dev@localhost` (password: `dev`)
- Root password: `root`

## Toolchain

The flake pins:

- **Rust 1.94.0** via `rust-overlay` (matches kernel.org recommendation for 6.18)
- **LLVM** from nixpkgs default (currently 21; clang, libclang, bindgen all match)
- **Linux 6.18** source from `github:torvalds/linux/v6.18`

## CI

The `integration-tests.yml` workflow runs `nix run .#test` on a self-hosted
runner. The runner needs the same host requirements listed above.

## Podman Initrd

The podman initrd (`nix build .#podmanInitrd`) is built entirely from nixpkgs
packages (podman, crun, netavark, etc.). The Nix store closure is copied into
the rootfs with FHS symlinks so the init script and containers find their tools.
No Docker, proot, or apt involved — the build is fully reproducible.
