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
  boot.extraModulePackages = [ bedrockModule ];
  boot.kernelModules = [ "bedrock" ];

  # Keep the EC2/Nitro initrd driver set from amazon-image.nix and add the
  # filesystems used by this disko layout.
  boot.initrd.availableKernelModules = [
    "nvme"
    "ena"
    "ahci"
    "sd_mod"
    "xhci_pci"
  ];
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
