#!/usr/bin/env python3
"""Generates a minimal, valid solid-color PNG icon for the Tauri app,
using only Python's standard library (zlib + struct) — no image library
needed. tauri-build wants a real, parseable icon file to exist at build
time; this produces one without requiring `cargo tauri icon` (which
itself needs the tauri-cli this sandbox can't run — see Phase 7 notes)."""
import struct
import zlib

def make_png(path, size=256, rgba=(79, 184, 222, 255)):  # the UI's cyan accent
    width = height = size
    raw = bytearray()
    for _y in range(height):
        raw.append(0)  # filter type: None
        for _x in range(width):
            raw.extend(rgba)

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data +
                struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)  # 8-bit RGBA
    idat = zlib.compress(bytes(raw), 9)

    with open(path, "wb") as f:
        f.write(sig)
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", idat))
        f.write(chunk(b"IEND", b""))

if __name__ == "__main__":
    make_png("desktop/src-tauri/icons/icon.png")
    print("Wrote desktop/src-tauri/icons/icon.png")
