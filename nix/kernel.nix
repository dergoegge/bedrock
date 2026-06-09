# Custom Linux 6.18 kernel with CONFIG_RUST=y
#
# Uses linuxManualConfig for full control over the .config.
# The kernel config is generated from nixpkgs' stock x86_64 NixOS kernel
# config plus the small set of Bedrock-specific overrides.
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

  # Seed 6.18 with nixpkgs' stock NixOS x86_64 6.18 kernel config. This keeps
  # physical platform support, AWS/Nitro drivers, filesystems, crypto, RAS, and
  # other distro defaults aligned with nixpkgs' supported 6.18 profile.
  stockConfigfile = pkgs.linuxPackages_6_18.kernel.configfile;

  # Generate a kernel .config from the stock NixOS config + required Bedrock
  # overrides. This avoids committing a full .config while staying close to
  # nixpkgs' supported kernel profile.
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

    # Start from nixpkgs' stock x86_64 config instead of upstream defconfig.
    cp ${stockConfigfile} .config

    # Bedrock is a Rust kernel module.
    ./scripts/config --enable RUST

    # Keep stock virtualization/KVM modules available. AWS hosts blacklist KVM
    # at runtime while Bedrock owns VMX, but local builders/tests still need
    # KVM to launch VMs.
    ./scripts/config --enable VIRTUALIZATION

    # Module support
    ./scripts/config --enable MODULES
    ./scripts/config --enable MODULE_UNLOAD

    # Don't treat warnings as errors (matches normal dev builds)
    ./scripts/config --disable WERROR

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
