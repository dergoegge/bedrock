# Out-of-tree build for the bedrock-io guest kernel module.
# Build with:
#   make -C $KERNEL_BUILD_DIR M=$(pwd) modules
# (KERNEL_BUILD_DIR points at the configured guest kernel build tree.)
#
# The Nix wrapper in nix/podman-initrd.nix sets these for us; this
# top-level Makefile is the conventional entry point for kbuild.

KDIR ?= /lib/modules/$(shell uname -r)/build

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean
