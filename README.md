# Restora

A file recovery and secure-erase tool, built from a raw architecture document up through a working cross-platform desktop application — FAT32 and NTFS metadata parsing, signature-based carving, a resumable scan-and-recovery pipeline, and free-space secure erasure with self-verification.

[![CI](https://github.com/optilinkboyz/restora/actions/workflows/ci.yml/badge.svg)](https://github.com/optilinkboyz/restora/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## What it does

- **Recovers deleted files** two independent ways:
  - **Metadata parsing** — reads FAT32 directory entries or NTFS MFT records directly to find deleted files with their original name, size, and (for NTFS) a `$Bitmap`-verified confidence score
  - **Signature carving** — scans raw disk bytes for known file-format signatures (JPEG, PNG, PDF, ZIP), recovering files even when all filesystem metadata is gone — a formatted drive, a deleted directory, a filesystem with no parser written for it yet
- **Securely erases free space** — overwrites only unallocated clusters (verified via the FAT or `$Bitmap`, never touching live files) with a configurable pattern (zero, random, or DoD 5220.22-M 3-pass), then self-verifies by re-running the carver against the wiped region
- **Persists scan sessions** to SQLite — a scan's results survive closing the app and reopening it later, in a different process
- **Ships as both a CLI and a desktop app** — `restora-cli` for scripting/automation, a Tauri-based GUI for interactive use

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  UI LAYER — Tauri desktop shell (desktop/) + restora-cli         │
├─────────────────────────────────────────────────────────────────┤
│  APPLICATION LAYER (restora-application)                         │
│  ScanSession · RecoveryJob · WipeJob · EventBus · SessionStore   │
├─────────────────────────────────────────────────────────────────┤
│  DOMAIN LAYER (restora-domain) — pure logic, no I/O               │
│  FilesystemParser (Fat32Parser, NtfsParser) · Carver              │
│  RecoverableFile · WipePattern                                   │
├─────────────────────────────────────────────────────────────────┤
│  INFRASTRUCTURE LAYER (restora-infra) — the only I/O layer        │
│  ByteSource · ImageFileSource · RawDiskSource                    │
│  WritableByteSource · WritableImageFileSource · privilege checks │
└─────────────────────────────────────────────────────────────────┘
```

The core design rule holding this together: everything above the infrastructure layer talks in terms of the `ByteSource` trait, never a concrete file handle or device path. A `FilesystemParser` or `Carver` reading a real physical drive and one reading a 16MB test `.img` file run through the exact same code — this is what let the entire parsing/carving/application layer be built and fully tested against plain files, with real block-device support (`RawDiskSource`, privilege-gated) added at the very end without touching anything else.

Read access (`ByteSource`) and write access (`WritableByteSource`) are deliberately separate traits, not one trait with an optional write method — code that only ever receives a `&dyn ByteSource` (which is all of scanning, parsing, and carving) has no way to write, at the type level, regardless of what the concrete type underneath could otherwise do. Only the wipe job ever asks for the writable trait.

## Project structure

```
restora/
├── crates/
│   ├── restora-infra/         # Raw I/O: ByteSource, RawDiskSource, privilege checks
│   ├── restora-domain/        # Parsers, carver, wipe patterns — pure logic
│   ├── restora-application/   # ScanSession, RecoveryJob, WipeJob, SessionStore
│   └── restora-api/           # Typed IPC boundary the desktop UI calls into
├── cli/                       # restora-cli — scriptable command-line interface
├── desktop/                   # Tauri desktop app (separate Cargo project — see below)
│   ├── src-tauri/
│   └── frontend/
├── scripts/                   # Test fixture generators
├── tests/fixtures/            # Generated .img test images (gitignored, not committed)
└── .github/workflows/ci.yml   # Cross-platform CI matrix
```

**Why `desktop/` is a separate Cargo project, not a workspace member**: Tauri 2.x's dependency tree requires a Rust 2024-edition-aware toolchain (rustc 1.85+). Keeping it outside the main workspace means the core logic (`crates/` + `cli/`) stays buildable anywhere with a reasonably current Rust install, independent of whatever toolchain constraints the desktop shell's dependencies impose.

## Getting started

### Prerequisites

- Rust via [rustup](https://rustup.rs) (not your OS package manager — you want a current toolchain)
- For fixture generation: `dosfstools` and `mtools` (Linux/WSL: `sudo apt install dosfstools mtools`), Python 3 (stdlib only, no extra packages)
- For the desktop app additionally: Node.js, the Tauri CLI (`cargo install tauri-cli --version "^2.0"`), and platform system dependencies — see [`desktop/SIGNING.md`](desktop/SIGNING.md) and the CI workflow for the exact Linux package list

### Build and test the core workspace

```bash
git clone https://github.com/optilinkboyz/restora.git
cd restora

# Generate test fixtures (not committed to the repo — regenerate locally)
chmod +x scripts/*.sh
./scripts/make_fat32_fixture.sh
./scripts/make_fat32_nested_fixture.sh
./scripts/make_carve_fixture.sh
python3 scripts/make_ntfs_fixture.py

cargo build
cargo test
```

### Run the desktop app

```bash
cd desktop/src-tauri
cargo tauri dev
```

## Using the CLI

```bash
# Scan an image (auto-detects FAT32 or NTFS)
restora-cli scan tests/fixtures/ntfs_basic.img

# Recover a specific file by name
restora-cli recover tests/fixtures/ntfs_basic.img canary.txt ./recovered/

# Signature-based carving (works with no filesystem at all)
restora-cli carve tests/fixtures/carve_test.img ./carved/

# Persisted, resumable scan session
restora-cli session-scan tests/fixtures/ntfs_basic.img sessions.db deep
restora-cli session-list sessions.db
restora-cli session-recover sessions.db <session-id> canary.txt ./recovered/

# Secure-erase free space (destructive — requires typing WIPE to confirm)
restora-cli wipe-free-space tests/fixtures/ntfs_basic.img zero --verify

# Real physical devices (requires root/Administrator)
sudo restora-cli scan /dev/sdb          # Linux
restora-cli scan \\.\PhysicalDrive1     # Windows, as Administrator
```

## Known limitations

Named explicitly rather than left implicit:

- **NTFS results show a flat filename, not a full path** — the parser scans MFT records directly rather than walking `$INDEX_ROOT` directory structures to reconstruct parent paths
- **FAT32 deleted filenames lose their first character** — the `0xE5` delete marker overwrites it; file *content* is unaffected
- **No ext4 or APFS support** — the architecture's `FilesystemParser` trait was designed to support them, but only FAT32 and NTFS are implemented
- **No true byte-offset scan resumability** — a cancelled Deep scan restarts carving from the beginning rather than continuing from where it stopped; only the *results already found* persist across a session reload
- **Secure erase only supports `.img` files, not real devices** — `RawDiskSource` (read) exists for physical devices, but there's no `WritableRawDiskSource` counterpart yet
- **No real TRIM/Discard support** — SSD-safe erasure currently means refusing to run an overwrite-based wipe (`--assume-ssd`), not issuing an actual TRIM command
- **Code signing is documented, not implemented** — see [`desktop/SIGNING.md`](desktop/SIGNING.md); it needs real certificates this project doesn't have

## Testing philosophy

Every parser and carver is tested two ways: hand-crafted byte arrays for the tricky low-level decoding logic (NTFS fixups, data-run parsing, FAT directory entries), and real generated disk images for end-to-end recovery proof — byte-exact comparison against known-original content, not just "did it not crash." The [CI workflow](.github/workflows/ci.yml) runs the full suite on Linux and a cross-platform compile check plus a portable test subset on Windows and macOS.

## License

MIT — see [LICENSE](LICENSE).
