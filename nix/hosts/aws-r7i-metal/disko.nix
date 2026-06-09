{ lib, ... }:

{
  disko.devices.disk.root = {
    type = "disk";
    device = lib.mkDefault "/dev/nvme0n1";
    content = {
      type = "gpt";
      partitions = {
        # Match Debian/AWS cloud image geometry closely: the boot support
        # partitions live at the front of the disk, while the root partition
        # is GPT entry 1 and starts at 128MiB.
        root = {
          priority = 1;
          start = "262144s";
          end = "-0";
          type = "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709";
          content = {
            type = "filesystem";
            format = "ext4";
            extraArgs = [ "-F" "-L" "nixos" ];
            mountpoint = "/";
          };
        };
        bios = {
          priority = 2;
          start = "2048s";
          end = "8191s";
          type = "EF02";
        };
        esp = {
          priority = 3;
          start = "8192s";
          end = "262143s";
          type = "EF00";
          content = {
            type = "filesystem";
            format = "vfat";
            extraArgs = [ "-F" "32" "-n" "ESP" ];
            mountpoint = "/boot/efi";
            mountOptions = [ "umask=0077" ];
          };
        };
      };
    };
  };
}
