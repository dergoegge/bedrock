# Minimal guest initramfs for bedrock VM tests.
# Init boots, immediately issues VMCALL shutdown to bedrock.
{ pkgs }:

let
  initBin = pkgs.stdenv.mkDerivation {
    name = "bedrock-guest-init";
    dontUnpack = true;
    buildPhase = ''
      cat > init.c << 'EOF'
      static inline void bedrock_shutdown(void) {
          __asm__ volatile(
              "mov $0, %%rax\n\t"
              "vmcall\n\t"
              :
              :
              : "rax"
          );
      }
      void _start(void) {
          bedrock_shutdown();
          for (;;) __asm__ volatile("hlt");
      }
      EOF
      $CC -static -nostdlib -o init init.c
    '';
    installPhase = "cp init $out";
  };
in
pkgs.runCommand "bedrock-guest-rootfs" {
  nativeBuildInputs = [ pkgs.cpio pkgs.gzip ];
} ''
  mkdir -p root
  cp ${initBin} root/init
  chmod +x root/init
  cd root
  find . -print0 | cpio --null -o -H newc | gzip -9 > $out
''
