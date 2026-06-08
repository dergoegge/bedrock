{ lib, ... }:

{
  disko.devices.disk.root = {
    type = "disk";
    device = lib.mkDefault "/dev/nvme0n1";
    content = {
      type = "gpt";
      partitions = {
        bios = {
          size = "1M";
          type = "EF02";
          priority = 1;
        };
        root = {
          size = "100%";
          priority = 2;
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
