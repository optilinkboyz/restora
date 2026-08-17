#!/bin/bash
# Builds a raw byte image with NO filesystem whatsoever — just a
# JPEG-signature blob and a PDF-signature blob sitting in a sea of zero
# padding. This is the scenario carving exists for: recovery with zero
# dependence on directory entries, MFT records, or any other metadata.
set -euo pipefail

OUT="tests/fixtures/carve_test.img"

: > "$OUT"

echo "Writing leading padding..."
dd if=/dev/zero bs=1 count=5000 >> "$OUT" 2>/dev/null

echo "Embedding a JPEG-signature blob (header FFD8FF ... footer FFD9)..."
printf '\xFF\xD8\xFF' >> "$OUT"
dd if=/dev/zero bs=1 count=500 2>/dev/null | tr '\0' '\252' >> "$OUT"   # 500 bytes of 0xAA filler
printf '\xFF\xD9' >> "$OUT"

echo "Writing a gap..."
dd if=/dev/zero bs=1 count=2000 >> "$OUT" 2>/dev/null

echo "Embedding a PDF-signature blob (header %PDF- ... footer %%EOF)..."
printf '%s' '%PDF-1.4
Fake pdf content for carving test.
%%EOF' >> "$OUT"

echo "Writing trailing padding..."
dd if=/dev/zero bs=1 count=3000 >> "$OUT" 2>/dev/null

echo ""
echo "Fixture ready: $OUT  (this image has NO filesystem at all — try"
echo "'restora-cli scan' on it and note it correctly finds nothing there,"
echo "then 'restora-cli carve' and note it finds both embedded blobs.)"
ls -la "$OUT"
