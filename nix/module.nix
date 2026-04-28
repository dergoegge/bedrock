# bedrock.ko kernel module derivation
#
# Builds the out-of-tree Rust/C/asm kernel module using Kbuild.
# Requires a kernel built with CONFIG_RUST=y.
{ pkgs
, kernel
, rustToolchain
, clippy ? false
, nestedVirt ? false
}:

let
  llvmPackages = pkgs.llvmPackages;
in
# Use LLVM stdenv to match the kernel build. The default stdenv's clang
# wrapper adds -Werror=unused-command-line-argument which breaks kernel builds.
llvmPackages.stdenv.mkDerivation {
  pname = "bedrock-module";
  version = "0.1.0";

  # Use the whole repo so symlinks in crates/bedrock/ resolve correctly.
  # (e.g. ept -> ../bedrock-ept/src, vmx -> ../bedrock-vmx/src)
  src = ./..;

  nativeBuildInputs = [
    rustToolchain
    pkgs.rust-bindgen
    llvmPackages.lld
    pkgs.gnumake
  ];

  # Don't run fixup phases that break kernel modules
  dontStrip = true;
  dontPatchELF = true;

  # Nix's clang wrapper adds -Werror=unused-command-line-argument globally.
  # The kernel passes -nostdlibinc which is unused in some contexts (asm).
  # Suppress it for all compiler invocations via the nix wrapper's env var.
  NIX_CFLAGS_COMPILE = "-Wno-unused-command-line-argument";

  # Let the kernel's Makefile handle CC/LD via LLVM=1.
  # Don't pass CC=clang explicitly -- avoids nix wrapper issues.
  buildPhase = ''
    cd crates/bedrock
    make \
      KDIR=${kernel.dev}/lib/modules/${kernel.modDirVersion}/build \
      LLVM=1 \
      ${pkgs.lib.optionalString clippy "CLIPPY=1 KRUSTFLAGS='-D warnings'"} \
      ${pkgs.lib.optionalString nestedVirt "NESTED_VIRT=1"}
  '';

  installPhase = ''
    mkdir -p $out/lib/modules/${kernel.modDirVersion}/extra
    cp bedrock.ko $out/lib/modules/${kernel.modDirVersion}/extra/
  '';

  meta = {
    description = "Bedrock hypervisor kernel module";
    license = pkgs.lib.licenses.gpl2Only;
    platforms = [ "x86_64-linux" ];
  };
}
