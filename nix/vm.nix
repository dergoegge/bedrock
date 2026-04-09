# NixOS dev VM configuration for interactive bedrock testing.
#
# Boot with: nix run .#vm
# SSH in with: ssh -p 2222 dev@localhost (password: dev)
#
# The VM runs the custom 6.18 kernel with the bedrock module loaded.
# Nested KVM is enabled so the bedrock hypervisor can use VMX.
{ pkgs, bedrockKernel, bedrockModule, bedrockCli, bedrockDeterminism }:

let
  nixos = pkgs.nixos ({ config, pkgs, modulesPath, ... }: {
    imports = [
      "${modulesPath}/virtualisation/qemu-vm.nix"
    ];

    boot.kernelPackages = pkgs.linuxPackagesFor bedrockKernel;
    boot.extraModulePackages = [ bedrockModule ];
    boot.kernelModules = [ "bedrock" ];

    # Our custom kernel builds most drivers built-in, not as modules.
    # Override NixOS defaults that expect loadable modules.
    boot.initrd.includeDefaultModules = false;
    boot.initrd.availableKernelModules = pkgs.lib.mkForce [ ];
    boot.initrd.kernelModules = pkgs.lib.mkForce [ ];

    # No KVM needed -- bedrock is the hypervisor.
    # The host runs KVM with nested VMX so bedrock can use VMX in the guest.

    # Ensure /dev/bedrock is world-accessible for testing
    services.udev.extraRules = ''
      KERNEL=="bedrock", MODE="0666"
    '';

    # SSH access
    services.openssh = {
      enable = true;
      settings.PermitRootLogin = "yes";
    };

    # Test user
    users.users.dev = {
      isNormalUser = true;
      password = "dev";
      extraGroups = [ "kvm" "wheel" ];
    };
    users.users.root.password = "root";

    # Bedrock tools + useful utilities
    environment.systemPackages = [
      bedrockCli
      bedrockDeterminism
      pkgs.just
      pkgs.strace
      pkgs.gdb
      pkgs.pciutils
    ];

    # VM hardware settings
    virtualisation = {
      cores = 4;
      memorySize = 8192;
      diskSize = 4096;
      # Share the project directory into the VM
      sharedDirectories.bedrock = {
        source = toString ./..;
        target = "/home/dev/bedrock";
      };
      qemu.options = [
        "-enable-kvm"
        "-cpu" "host"
        "-nographic"
      ];
      graphics = false;
    };

    # Forward SSH port
    virtualisation.forwardPorts = [
      { from = "host"; host.port = 2222; guest.port = 22; }
    ];

    networking.hostName = "bedrock-vm";
    system.stateVersion = "24.11";
  });
in
nixos.config.system.build.vm
