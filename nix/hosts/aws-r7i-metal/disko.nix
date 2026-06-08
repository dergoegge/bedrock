{ lib, ... }:

{
  disko.devices.disk.root = {
    type = "disk";
    device = lib.mkDefault "/dev/nvme0n1";
    content = {
      type = "gpt";
      postCreateHook = ''
        parted /dev/nvme0n1 disk_set pmbr_boot on
      '';
      partitions = {
        bios = {
          size = "1M";
          type = "EF02";
          priority = 1;
        };
        boot = {
          size = "1G";
          priority = 2;
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [ "-F" "-L" "boot" ];
            mountpoint = "/boot";
          };
        };
        root = {
          size = "100%";
          priority = 3;
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
