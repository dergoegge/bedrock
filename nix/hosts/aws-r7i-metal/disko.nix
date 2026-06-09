{ lib, ... }:

{
  disko.devices.disk.root = {
    type = "disk";
    device = lib.mkDefault "/dev/nvme0n1";
    content = {
      type = "gpt";
      partitions = {
        # Match the working NixOS community AWS AMI layout:
        # p1: 1MiB BIOS boot partition, p2: ext4 root labeled nixos.
        bios = {
          priority = 1;
          start = "2048s";
          end = "4095s";
          type = "EF02";
          label = "no-fs";
        };
        root = {
          priority = 2;
          start = "4096s";
          end = "-34s";
          type = "8300";
          label = "primary";
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [ "-F" "-L" "nixos" ];
            mountpoint = "/";
          };
        };
      };
    };
  };
}
