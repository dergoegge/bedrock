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

    # No GPU/display stack is needed on AWS bare metal or Bedrock hosts.
    ./scripts/config --disable DRM
    ./scripts/config --disable DRM_I915
    ./scripts/config --disable DRM_XE
    ./scripts/config --disable DRM_AMDGPU
    ./scripts/config --disable DRM_RADEON
    ./scripts/config --disable DRM_NOUVEAU
    ./scripts/config --disable AGP
    ./scripts/config --disable VGA_ARB
    ./scripts/config --disable VGA_SWITCHEROO
    ./scripts/config --disable FB
    ./scripts/config --disable BACKLIGHT_CLASS_DEVICE
    ./scripts/config --disable BACKLIGHT_KTD253
    ./scripts/config --disable BACKLIGHT_KTD2801
    ./scripts/config --disable BACKLIGHT_KTZ8866
    ./scripts/config --disable BACKLIGHT_LM3533
    ./scripts/config --disable BACKLIGHT_PWM
    ./scripts/config --disable BACKLIGHT_MT6370
    ./scripts/config --disable BACKLIGHT_APPLE
    ./scripts/config --disable BACKLIGHT_QCOM_WLED
    ./scripts/config --disable BACKLIGHT_RT4831
    ./scripts/config --disable BACKLIGHT_SAHARA
    ./scripts/config --disable BACKLIGHT_ADP8860
    ./scripts/config --disable BACKLIGHT_ADP8870
    ./scripts/config --disable BACKLIGHT_LM3509
    ./scripts/config --disable BACKLIGHT_LM3630A
    ./scripts/config --disable BACKLIGHT_LM3639
    ./scripts/config --disable BACKLIGHT_LP855X
    ./scripts/config --disable BACKLIGHT_MP3309C
    ./scripts/config --disable BACKLIGHT_SKY81452
    ./scripts/config --disable BACKLIGHT_TPS65217
    ./scripts/config --disable BACKLIGHT_GPIO
    ./scripts/config --disable BACKLIGHT_LV5207LP
    ./scripts/config --disable BACKLIGHT_BD6107
    ./scripts/config --disable BACKLIGHT_ARCXCNN
    ./scripts/config --disable BACKLIGHT_RAVE_SP
    ./scripts/config --disable BACKLIGHT_LED
    ./scripts/config --disable LEDS_TRIGGER_BACKLIGHT
    ./scripts/config --disable CROS_KBD_LED_BACKLIGHT
    ./scripts/config --disable NVIDIA_WMI_EC_BACKLIGHT
    ./scripts/config --disable DELL_UART_BACKLIGHT

    # Other headless-host trims.
    ./scripts/config --disable SOUND
    ./scripts/config --disable SND
    ./scripts/config --disable WIRELESS
    ./scripts/config --disable WLAN
    ./scripts/config --disable CFG80211
    ./scripts/config --disable MAC80211
    ./scripts/config --disable RFKILL
    ./scripts/config --disable BLUETOOTH
    ./scripts/config --disable BT
    ./scripts/config --disable MEDIA_SUPPORT
    ./scripts/config --disable DVB_CORE
    ./scripts/config --disable RC_CORE

    # This host class is Intel-only.
    ./scripts/config --disable CPU_SUP_AMD
    ./scripts/config --disable X86_AMD_PLATFORM_DEVICE
    ./scripts/config --disable X86_MCE_AMD
    ./scripts/config --disable PERF_EVENTS_AMD_POWER
    ./scripts/config --disable PERF_EVENTS_AMD_UNCORE
    ./scripts/config --disable PERF_EVENTS_AMD_BRS
    ./scripts/config --disable X86_AMD_PSTATE
    ./scripts/config --disable X86_AMD_PSTATE_UT
    ./scripts/config --disable X86_AMD_FREQ_SENSITIVITY
    ./scripts/config --disable AMD_MEM_ENCRYPT
    ./scripts/config --disable AMD_NUMA
    ./scripts/config --disable AMD_NB
    ./scripts/config --disable AMD_NODE
    ./scripts/config --disable KVM_AMD
    ./scripts/config --disable KVM_AMD_SEV
    ./scripts/config --disable AMD_IOMMU
    ./scripts/config --disable EDAC_AMD64
    ./scripts/config --disable AMD_HSMP
    ./scripts/config --disable AMD_HSMP_ACPI
    ./scripts/config --disable AMD_HSMP_PLAT
    ./scripts/config --disable AMD_PMF
    ./scripts/config --disable AMD_PMC
    ./scripts/config --disable AMD_HFI
    ./scripts/config --disable AMD_3D_VCACHE
    ./scripts/config --disable AMD_WBRF
    ./scripts/config --disable AMD_ISP_PLATFORM
    ./scripts/config --disable AMD_ATL
    ./scripts/config --disable AMDTEE
    ./scripts/config --disable NET_VENDOR_AMD
    ./scripts/config --disable AMD8111_ETH
    ./scripts/config --disable AMD_XGBE
    ./scripts/config --disable AMD_PHY
    ./scripts/config --disable MTD_CFI_AMDSTD
    ./scripts/config --disable MTD_AMD76XROM
    ./scripts/config --disable PATA_AMD
    ./scripts/config --disable PATA_ATIIXP
    ./scripts/config --disable HW_RANDOM_AMD
    ./scripts/config --disable I2C_AMD756
    ./scripts/config --disable I2C_AMD8111
    ./scripts/config --disable I2C_AMD_MP2
    ./scripts/config --disable I2C_AMD_ASF
    ./scripts/config --disable SPI_AMD
    ./scripts/config --disable PINCTRL_AMD
    ./scripts/config --disable GPIO_AMDPT
    ./scripts/config --disable GPIO_AMD_FCH
    ./scripts/config --disable GPIO_AMD8111
    ./scripts/config --disable W1_MASTER_AMD_AXI
    ./scripts/config --disable AMD_SFH_HID
    ./scripts/config --disable USB_AMD5536UDC
    ./scripts/config --disable AMD_AE4DMA
    ./scripts/config --disable AMD_PTDMA
    ./scripts/config --disable AMD_QDMA
    ./scripts/config --disable NTB_AMD
    ./scripts/config --disable CRYPTO_DEV_CCP

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
