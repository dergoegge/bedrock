#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Usage: make-initrd.sh [initrd-dir] [output-file]
# Default initrd-dir: scripts/initrd
# Default output: initrd.cpio.gz (or initrd-podman.cpio.gz for podman variant)
INITRD_DIR="${1:-initrd}"
INITRD_PATH="$SCRIPT_DIR/$INITRD_DIR"

if [[ ! -d "$INITRD_PATH" ]]; then
    echo "Error: $INITRD_PATH does not exist"
    echo "Available options:"
    ls -d "$SCRIPT_DIR"/initrd* 2>/dev/null | xargs -n1 basename
    exit 1
fi

# Default output name based on initrd dir
if [[ "$INITRD_DIR" == "initrd" ]]; then
    DEFAULT_OUTPUT="$PROJECT_DIR/initrd.cpio.gz"
else
    DEFAULT_OUTPUT="$PROJECT_DIR/${INITRD_DIR}.cpio.gz"
fi
OUTPUT="${2:-$DEFAULT_OUTPUT}"

IMAGE_NAME="bedrock-$INITRD_DIR"
CONTAINER_NAME="bedrock-$INITRD_DIR-temp"

cd "$INITRD_PATH"

echo "Building initrd from: $INITRD_PATH"
echo "Output: $OUTPUT"
echo ""
echo "Building image..."
docker build --network=host -t "$IMAGE_NAME" .

echo "Creating container..."
docker rm -f "$CONTAINER_NAME" 2>/dev/null || true
docker create --name "$CONTAINER_NAME" "$IMAGE_NAME"

echo "Exporting filesystem..."
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR; docker rm -f $CONTAINER_NAME 2>/dev/null || true" EXIT

docker export "$CONTAINER_NAME" | tar -C "$TMPDIR" -xf -

echo "Creating initramfs..."
(cd "$TMPDIR" && find . | cpio -o -H newc 2>/dev/null | gzip -9) > "$OUTPUT"

echo "Created: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
