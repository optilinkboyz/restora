#!/bin/bash
# Builds a FAT32 image with a subdirectory (SUBDIR) containing a file that
# gets deleted — used to test recursive directory-tree walking, as opposed
# to fat32_basic.img which only has a root-level deleted file.
set -euo pipefail

OUT="tests/fixtures/fat32_nested.img"

echo "Creating 16MB blank image..."
dd if=/dev/zero of="$OUT" bs=1M count=16 status=none

echo "Formatting as FAT32..."
mkfs.fat -F 32 -n RESTORATEST "$OUT" > /dev/null

echo "Creating subdirectory..."
mmd -i "$OUT" ::SUBDIR

echo "Writing a file inside the subdirectory..."
echo "Nested file inside a subdirectory, for testing recursive directory traversal." > /tmp/deep_original.txt
mcopy -i "$OUT" /tmp/deep_original.txt ::SUBDIR/deep.txt

echo "Confirming layout before delete:"
mdir -i "$OUT" ::SUBDIR

echo "Deleting the nested file..."
mdel -i "$OUT" ::SUBDIR/deep.txt

echo "Confirming it's gone:"
mdir -i "$OUT" ::SUBDIR

echo ""
echo "Fixture ready: $OUT"
ls -la "$OUT"
