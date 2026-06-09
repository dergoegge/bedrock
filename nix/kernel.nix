# Custom Linux 6.18 kernel with CONFIG_RUST=y
#
# Uses linuxManualConfig for full control over the .config.
# The kernel config is generated from defconfig + overrides.
{ pkgs
, linux-src
, rustToolchain
}:

let
  version = "6.18.0";
  modDirVersion = "6.18.0";

  # Use nixpkgs' default LLVM so clang, libclang, and bindgen all match.
  # kernel.org ships LLVM 22 for 6.18, so any recent LLVM works.
  llvmPackages = pkgs.llvmPackages;

  # Generate a kernel .config from defconfig + required overrides.
  # This avoids committing a full .config that drifts with kernel versions.
  configfile = pkgs.runCommand "linux-6.18-config" {
    nativeBuildInputs = [
      llvmPackages.clang
      llvmPackages.llvm
      llvmPackages.lld
      rustToolchain
      pkgs.rust-bindgen
      pkgs.python3
      pkgs.gnumake
      pkgs.flex
      pkgs.bison
      pkgs.bc
      pkgs.perl
      pkgs.elfutils
      pkgs.openssl
    ];
  } ''
    cp -r ${linux-src} src
    chmod -R u+w src
    cd src

    # Fix shebangs (scripts use #!/usr/bin/env which doesn't exist in sandbox)
    patchShebangs scripts/

    # Start from defconfig (x86_64)
    make LLVM=1 ARCH=x86 defconfig

    # Enable Rust support
    ./scripts/config --enable RUST

    # Virtualization support (but NOT KVM -- bedrock replaces it)
    ./scripts/config --enable VIRTUALIZATION
    ./scripts/config --disable KVM
    ./scripts/config --disable KVM_INTEL

    # Module support
    ./scripts/config --enable MODULES
    ./scripts/config --enable MODULE_UNLOAD
    ./scripts/config --enable MODULE_FORCE_LOAD

    # Physical x86_64 platform support. AWS r7i.metal boots via legacy BIOS,
    # but the Nitro platform still relies on the normal ACPI/DMI/PCI stack.
    ./scripts/config --enable ACPI
    ./scripts/config --enable ACPI_PROCESSOR
    ./scripts/config --enable ACPI_HOTPLUG_CPU
    ./scripts/config --enable ACPI_THERMAL
    ./scripts/config --enable ACPI_BUTTON
    ./scripts/config --enable DMI
    ./scripts/config --enable DMIID
    ./scripts/config --enable DMI_SYSFS
    ./scripts/config --enable PCI
    ./scripts/config --enable PCI_MSI
    ./scripts/config --enable PCIEPORTBUS
    ./scripts/config --enable HOTPLUG_PCI

    # Early userspace and disk discovery requirements.
    ./scripts/config --enable DEVTMPFS
    ./scripts/config --enable DEVTMPFS_MOUNT
    ./scripts/config --enable BLK_DEV_INITRD
    ./scripts/config --enable PARTITION_ADVANCED
    ./scripts/config --enable EFI_PARTITION
    ./scripts/config --enable MSDOS_PARTITION
    ./scripts/config --enable EFI
    ./scripts/config --enable EFI_STUB
    ./scripts/config --enable EFI_RUNTIME_MAP

    # Base networking used by systemd-networkd after the initrd.
    ./scripts/config --enable NET
    ./scripts/config --enable UNIX
    ./scripts/config --enable INET
    ./scripts/config --enable PACKET

    # Virtio (needed for NixOS VM)
    ./scripts/config --enable VIRTIO
    ./scripts/config --enable VIRTIO_PCI
    ./scripts/config --enable VIRTIO_BLK
    ./scripts/config --enable VIRTIO_NET
    ./scripts/config --enable VIRTIO_CONSOLE
    ./scripts/config --enable VIRTIO_BALLOON
    ./scripts/config --enable HW_RANDOM_VIRTIO

    # AWS Nitro bare metal (r7i.metal): EBS appears as NVMe and networking is ENA.
    ./scripts/config --enable NVME_CORE
    ./scripts/config --enable BLK_DEV_NVME
    ./scripts/config --enable NET_VENDOR_AMAZON
    ./scripts/config --enable ENA_ETHERNET

    # 9P filesystem (for NixOS VM store sharing)
    ./scripts/config --enable NET_9P
    ./scripts/config --enable NET_9P_VIRTIO
    ./scripts/config --enable 9P_FS
    ./scripts/config --enable 9P_FS_POSIX_ACL

    # Serial console
    ./scripts/config --enable SERIAL_8250
    ./scripts/config --enable SERIAL_8250_CONSOLE
    ./scripts/config --enable SERIAL_8250_PNP
    ./scripts/config --enable SERIAL_8250_PCI
    ./scripts/config --enable EARLY_PRINTK
    ./scripts/config --enable MAGIC_SYSRQ
    ./scripts/config --enable PRINTK_TIME

    # Ext4 + tmpfs
    ./scripts/config --enable EXT4_FS
    ./scripts/config --enable TMPFS

    # Misc device support (bedrock registers as misc device)
    ./scripts/config --enable MISC_DEVICES

    # NixOS requirements
    ./scripts/config --enable OVERLAY_FS
    ./scripts/config --enable CRYPTO_USER_API_HASH
    ./scripts/config --enable SQUASHFS
    ./scripts/config --enable SQUASHFS_XZ
    ./scripts/config --enable SQUASHFS_ZSTD

    # Don't treat warnings as errors (matches normal dev builds)
    ./scripts/config --disable WERROR

    # Disable unnecessary features to speed up build
    ./scripts/config --disable SOUND
    ./scripts/config --disable DRM
    ./scripts/config --disable WIRELESS
    ./scripts/config --disable WLAN
    ./scripts/config --disable BLUETOOTH

    # Resolve any dependency issues
    make LLVM=1 ARCH=x86 olddefconfig

    # Verify Rust toolchain is usable by the kernel build
    make LLVM=1 ARCH=x86 rustavailable

    cp .config $out
  '';

  # Use .override to swap stdenv to LLVM -- this makes nixpkgs pass
  # CC=clang, LD=ld.lld, etc. on the make command line (which otherwise
  # hardcodes gcc paths that override any Makefile-level LLVM=1).
  base = (pkgs.linuxManualConfig {
    inherit version modDirVersion configfile;
    src = linux-src;
    allowImportFromDerivation = true;
  }).override {
    stdenv = llvmPackages.stdenv;
  };

in
base.overrideAttrs (old: {
  # IMPORTANT: rustToolchain must come FIRST to shadow any default rustc
  # that nixpkgs adds for CONFIG_RUST=y kernels.
  nativeBuildInputs = [
    rustToolchain
  ] ++ (old.nativeBuildInputs or []) ++ [
    pkgs.python3
    pkgs.elfutils
    pkgs.openssl
  ];

  # LLVM=1 is still needed for the kernel's internal logic (integrated
  # assembler, llvm-ar, etc.) beyond just CC/LD selection.
  postPatch = (old.postPatch or "") + ''
    sed -i '2iLLVM=1' Makefile
  '';
})
