{ lib, ... }:

{
  disko.devices.disk.root = {
    type = "disk";
    device = lib.mkDefault "/dev/nvme0n1";
    content = {
      type = "table";
      format = "msdos";
      partitions = [
        {
          name = "boot";
          start = "1M";
          end = "1025M";
          part-type = "primary";
          fs-type = "ext4";
          bootable = true;
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [ "-F" "-L" "boot" ];
            mountpoint = "/boot";
          };
        }
        {
          name = "root";
          start = "1025M";
          end = "100%";
          part-type = "primary";
          fs-type = "ext4";
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [ "-F" "-L" "nixos" ];
            mountpoint = "/";
          };
        }
      ];
    };
  };
}
