{ lib
, pkgs
, modulesPath
, bedrockKernel
, bedrockModule
, bedrockCli
, bedrockDeterminism
, useStockKernel ? false
, ...
}:

let
  bedrockKernelPackages = pkgs.linuxPackagesFor bedrockKernel;
in
{
  imports = [
    "${modulesPath}/virtualisation/amazon-image.nix"
    ./disko.nix
  ];

  boot.kernelPackages =
    if useStockKernel then pkgs.linuxPackages_6_12 else bedrockKernelPackages;
  # amazon-image.nix adds nixpkgs' out-of-tree ENA module, but that module
  # does not build against the pinned Linux 6.18 headers. ENA is enabled in
  # nix/kernel.nix, so only Bedrock needs to be installed as an extra module.
  boot.extraModulePackages = lib.mkForce (lib.optionals (!useStockKernel) [ bedrockModule ]);
  boot.kernelModules = lib.mkIf (!useStockKernel) [ "bedrock" ];

  boot.initrd = {
    supportedFilesystems = [ "ext4" "vfat" ];
  } // lib.optionalAttrs (!useStockKernel) {
    # The custom Bedrock kernel is intentionally slim and builds most required
    # drivers in-tree/built-in. Avoid NixOS' broad default initrd module list,
    # which includes modules this kernel does not build.
    includeDefaultModules = false;
    availableKernelModules = lib.mkForce [
      "nvme"
      "ena"
      "ahci"
      "sd_mod"
      "xhci_pci"
    ];
    kernelModules = lib.mkForce [ ];
  };

  # r7i.metal boots as EC2 bare metal on Nitro. AWS documents bare-metal
  # instances as an exception to the generic Nitro UEFI support, so use BIOS
  # GRUB on GPT with an EF02 BIOS boot partition from disko.nix.
  ec2.efi = false;
  boot.loader.grub.devices = lib.mkForce [ "/dev/nvme0n1" ];
  boot.loader.grub.useOSProber = false;

  # Keep EC2 metadata/key handling from amazon-image.nix, but do not let
  # instance user-data replace this flake-managed system after installation.
  virtualisation.amazon-init.enable = false;

  # Bedrock owns VMX. Do not load KVM on the deployed host.
  boot.blacklistedKernelModules = [
    "kvm"
    "kvm_intel"
    "kvm_amd"
    "nouveau"
    "xen_fbfront"
  ];

  services.udev.extraRules = ''
    KERNEL=="bedrock", MODE="0660", GROUP="bedrock"
  '';

  users.groups.bedrock = { };
  users.users.root.openssh.authorizedKeys.keys = lib.mkDefault [ ];

  environment.systemPackages = [
    bedrockCli
    bedrockDeterminism
    pkgs.gitMinimal
    pkgs.just
    pkgs.pciutils
    pkgs.strace
  ];

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "root" "@wheel" ];
  };

  system.stateVersion = "26.05";
}
