#!/usr/bin/env bash
# Helper script to extract specific pages from Intel SDM PDFs
# Usage: pdfpage.sh <volume> <start-page> [end-page]

SDM_DIR="$(cd "$(dirname "$0")" && pwd)"

# Volume mapping
declare -A VOLUMES=(
    ["1"]="253665-089-sdm-vol-1-1.pdf"
    ["2a"]="253666-089-sdm-vol-2a.pdf"
    ["2b"]="253667-089-sdm-vol-2b.pdf"
    ["2c"]="326018-089-sdm-vol-2c.pdf"
    ["2d"]="334569-089-sdm-vol-2d.pdf"
    ["3a"]="253668-089-sdm-vol-3a.pdf"
    ["3b"]="253669-089-sdm-vol-3b.pdf"
    ["3c"]="326019-089-sdm-vol-3c.pdf"
    ["3d"]="332831-089-sdm-vol-3d.pdf"
    ["4"]="335592-089-sdm-vol-4.pdf"
)

if [ $# -lt 2 ]; then
    echo "Usage: $0 <volume> <start-page> [end-page]"
    echo ""
    echo "Volumes: 1, 2a, 2b, 2c, 2d, 3a, 3b, 3c, 3d, 4"
    echo ""
    echo "Examples:"
    echo "  $0 3c 63       # Extract page 63 from Volume 3C"
    echo "  $0 3c 63 65    # Extract pages 63-65 from Volume 3C"
    exit 1
fi

VOLUME="$1"
START_PAGE="$2"
END_PAGE="${3:-$START_PAGE}"

# Validate volume
if [ -z "${VOLUMES[$VOLUME]}" ]; then
    echo "Error: Invalid volume '$VOLUME'"
    echo "Valid volumes: ${!VOLUMES[@]}"
    exit 1
fi

PDF_FILE="${SDM_DIR}/${VOLUMES[$VOLUME]}"

# Check if PDF exists
if [ ! -f "$PDF_FILE" ]; then
    echo "Error: PDF file not found: $PDF_FILE"
    exit 1
fi

# Extract pages using pdftotext
pdftotext -f "$START_PAGE" -l "$END_PAGE" -layout "$PDF_FILE" -
