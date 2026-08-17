#!/bin/bash
# Builds a small FAT32 disk image with a known file, then deletes it,
# for use in Phase 2 tests. Uses mtools (mcopy/mdel) instead of a kernel
# loop-mount, so it works identically whether you have root/mount
# capability or not — genuinely more portable for a CI environment too.
set -euo pipefail

OUT="tests/fixtures/fat32_basic.img"

echo "Creating 16MB blank image..."
dd if=/dev/zero of="$OUT" bs=1M count=16 status=none

echo "Formatting as FAT32..."
mkfs.fat -F 32 -n RESTORATEST "$OUT" > /dev/null

echo "Writing known test file (via mtools, no mount needed)..."
echo "This is a canary file for FAT32 recovery testing. If you can read this after carving, the recovery worked." > /tmp/canary_original.txt
mcopy -i "$OUT" /tmp/canary_original.txt ::canary.txt

echo "Confirming it's on the image:"
mdir -i "$OUT"

echo "Deleting it (normal delete — marks dir entry 0xE5, doesn't touch data)..."
mdel -i "$OUT" ::canary.txt

echo "Confirming it's gone from the directory listing:"
mdir -i "$OUT"

echo ""
echo "Fixture ready: $OUT"
ls -la "$OUT"
