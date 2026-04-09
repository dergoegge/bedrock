# Load environment variables from .env file
set dotenv-load

# Remote host for sync/build (configure in .env)
# Uses lazy evaluation so local commands work without remote config
remote_host := `echo ${REMOTE_HOST:-}`
remote_dir := `echo ${REMOTE_DIR:-}`

# Default: run tests
default: test

# Run tests
[group: 'local']
test:
    cargo test

# Format code
[group: 'local']
fmt:
    cargo fmt
    rustfmt --edition 2021 crates/bedrock/*.rs crates/bedrock/vm_file/*.rs

# Build the kernel module
[group: 'local']
build:
    make -C crates/bedrock

# Clean kernel module build artifacts
[group: 'local']
clean:
    make clean -C crates/bedrock

# Load the kernel module
[group: 'local']
load:
    sudo make load -C crates/bedrock

# Count lines of Rust code (excluding tests)
[group: 'local']
count-lines:
    find . -type f -name '*.rs' -not -path '*/.*/*' -not -path './target/*' -not -name '*test*' -not -lname '*' -exec cat {} + | wc -l

# Sync to remote
[group: 'remote']
sync:
    rsync -avz --delete --exclude '.git' --exclude '.claude' --exclude target ./ {{remote_host}}:{{remote_dir}}

# Build on remote (sync then build)
[group: 'remote']
remote: sync
    ssh {{remote_host}} 'cd {{remote_dir}} && just build'

# Clean remote build artifacts
[group: 'remote']
remote-clean:
    ssh {{remote_host}} 'cd {{remote_dir}} && just clean'

# Reload netconsole (configure NETCONSOLE_IP in .env)
[group: 'local']
netconsole:
    rmmod netconsole 2>/dev/null; modprobe netconsole netconsole=@/eno1,@`echo $NETCONSOLE_IP`/

# Boot NixOS dev VM with nested KVM
[group: 'nix']
vm:
    nix run .#vm

# Run NixOS integration tests in VM (requires KVM, slow due to nested virt)
[group: 'nix']
nix-test:
    nix run .#test

# Run tests natively on host (requires bedrock module loaded)
[group: 'nix']
nix-test-native:
    nix run .#test-native
