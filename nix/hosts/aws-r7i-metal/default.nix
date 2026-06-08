{ lib
, pkgs
, modulesPath
, bedrockKernel
, bedrockModule
, bedrockCli
, bedrockDeterminism
, ...
}:

{
  imports = [
    "${modulesPath}/virtualisation/amazon-image.nix"
    ./disko.nix
  ];

  boot.kernelPackages = pkgs.linuxPackagesFor bedrockKernel;
  # amazon-image.nix adds nixpkgs' out-of-tree ENA module, but that module
  # does not build against the pinned Linux 6.18 headers. ENA is enabled in
  # nix/kernel.nix, so only Bedrock needs to be installed as an extra module.
  boot.extraModulePackages = lib.mkForce [ bedrockModule ];
  boot.kernelModules = [ "bedrock" ];

  # The custom Bedrock kernel is intentionally slim and builds most required
  # drivers in-tree/built-in. Avoid NixOS' broad default initrd module list,
  # which includes modules this kernel does not build.
  boot.initrd.includeDefaultModules = false;
  boot.initrd.availableKernelModules = lib.mkForce [
    "nvme"
    "ena"
    "ahci"
    "sd_mod"
    "xhci_pci"
  ];
  boot.initrd.kernelModules = lib.mkForce [ ];
  boot.initrd.supportedFilesystems = [ "ext4" "vfat" ];

  # r7i.metal boots as EC2 bare metal on Nitro. AWS documents bare-metal
  # instances as an exception to the generic Nitro UEFI support, so use BIOS
  # GRUB on GPT with an EF02 BIOS boot partition from disko.nix.
  ec2.efi = false;
  boot.loader.grub.device = lib.mkForce "";
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
