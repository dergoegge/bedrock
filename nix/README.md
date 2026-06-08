# Nix Build System

Nix flake for building and testing the bedrock hypervisor with nested KVM.

## Quick Start

```bash
nix run .#vm            # Boot interactive dev VM (SSH on port 2222)
nix run .#test          # Run integration tests in NixOS VM
nix run .#test-native   # Run tests directly on host (faster, no nested virt)
nix run .#deploy-aws-r7i-metal -- root@EC2_IP
```

## Packages

| Package | Description |
|---------|-------------|
| `kernel` | Linux 6.18 with `CONFIG_RUST=y` (no KVM) |
| `bedrockModule` | `bedrock.ko` kernel module |
| `guestKernel` | Linux 6.18 with determinism patches (TLB flush) + `vmlinux` |
| `guestInitrd` | Trivial initramfs (boots, VMCALL shutdown) |
| `bedrock-cli` | CLI for loading and running guest VMs |
| `bedrock-determinism` | Determinism checker (multi-run comparison) |

## AWS Bare-Metal Deployment

The flake exposes `nixosConfigurations.aws-r7i-metal`, a
`nixos-anywhere` target for EC2 `r7i.metal-*` instances. It reuses
nixpkgs' EC2 module (`virtualisation/amazon-image.nix`) for AWS metadata,
SSM agent, serial console, ENA/NVMe defaults, and EC2 SSH behavior. The
only EC2 feature it disables is `amazon-init`, so EC2 user-data cannot
replace the flake-managed system after installation. Bedrock's custom Linux
6.18 kernel and `bedrock.ko` are layered on top.

The disk layout is declared with disko in
`nix/hosts/aws-r7i-metal/disko.nix`. It wipes `/dev/nvme0n1`, creates a
GPT disk with a BIOS boot partition, and formats `/` as ext4 labeled
`nixos`. This matches EC2 Nitro's NVMe EBS presentation and avoids UEFI
because AWS documents bare-metal instances as an exception to generic Nitro
UEFI support.

Deploy from a machine with SSH access to the instance:

```bash
BEDROCK_DEPLOY_PUBLIC_KEY="$(cat ~/.ssh/id_ed25519.pub)" \
  nix run .#deploy-aws-r7i-metal -- --build-on local root@EC2_PUBLIC_IP
```

The same helper is available through `just`:

```bash
BEDROCK_DEPLOY_PUBLIC_KEY="$(cat ~/.ssh/id_ed25519.pub)" \
  just deploy-aws-r7i-metal --build-on local root@EC2_PUBLIC_IP
```

The helper wraps:

```bash
nix run github:nix-community/nixos-anywhere -- \
  --flake .#aws-r7i-metal root@EC2_PUBLIC_IP
```

`BEDROCK_DEPLOY_PUBLIC_KEY` is optional but recommended. When set, the
helper passes it via `nixos-anywhere --extra-files` so the installed
system keeps that key in `/root/.ssh/authorized_keys` without embedding it
in the Nix store.

The target instance must already be booted into a Linux environment that
permits root SSH, such as a temporary NixOS or rescue-style EC2 instance.
`nixos-anywhere` will kexec into its installer, repartition the root EBS
volume, install NixOS, and reboot.

The podman initrd is exposed as a function `mkPodmanInitrd` under
`lib.<system>` — workloads supply a compose file and a docker-archive
tarball (from `docker save`) and the function returns a bootable initramfs:

```nix
let
  bedrock = inputs.bedrock;
  myInitrd = bedrock.lib.x86_64-linux.mkPodmanInitrd {
    composeYaml = ./compose.yaml;
    imagesTar   = ./images.tar;   # docker save img1 [img2 ...] -o images.tar
  };
in ...
```

Build any with `nix build .#<name>`.

## Host Requirements

### Nix Configuration (`/etc/nix/nix.conf`)

```
experimental-features = nix-command flakes
sandbox = relaxed
extra-sandbox-paths = /dev/kvm
```

- **`nix-command flakes`**: Required for `nix build`, `nix run`, etc.
- **`sandbox = relaxed`**: Needed if any of your workload's image builds use
  `dockerTools.pullImage` or other fixed-output derivations that pull from a
  network. With `sandbox = true`, all network is blocked. `relaxed` allows
  FODs to access the network while keeping other builds sandboxed. The
  initrd builder itself does not need network access — it only consumes the
  `images.tar` you supply.
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

The podman initrd is built entirely from nixpkgs packages (podman, crun,
netavark, journald, etc.). The Nix store closure is copied into the rootfs
with FHS symlinks so the init script and containers find their tools. The
only bedrock-specific bits the initrd ships are `bedrock-pebs-register`
(run at boot to enable precise EPT-friendly PEBS exits) and the
`bedrock-io.ko` kernel module (the deterministic I/O channel).

Workloads are everything else — compose file plus container images.
Anything workload-specific (helper binaries, driver scripts, configs) gets
baked into one of the images. Produce `images.tar` with whatever toolchain
you like (`docker build` + `docker save` outside Nix, or
`dockerTools.buildLayeredImage` inside Nix) and hand it to
`mkPodmanInitrd` along with the compose file.

### Workloads

Workloads live in `workloads/<name>/`. The flake auto-discovers every
subdirectory and exposes a `<name>Initrd` package once `images.tar` shows
up in the flake source. Build (and run / boot) a workload with:

```bash
just build-workload bitcoin    # prints the /nix/store/...-initrd path
```

`build-workload` runs the workload's `build.sh`, stages the resulting
`images.tar` (`git add -f`), invokes `nix build`, prints the output
path, then unstages on `EXIT` so the tarball can't end up in a commit by
accident — even if the build is interrupted. The tarball itself stays
gitignored.

For the bitcoin workload specifically, `build.sh` does `docker pull` +
two `docker build`s (a miner image FROM `bitcoin/bitcoin:latest` with
`bedrock-miner` baked in, and an alpine image with `bedrock-shutdown`
baked in), then `docker save`s all three images into one archive. See
`workloads/bitcoin/build.sh` and the per-image `Dockerfile`s.

CI (`.github/workflows/nix.yml`) runs the same `build.sh` + `git add -f`
steps before invoking the integration test — its checkout is ephemeral
so the unstage isn't needed there.
