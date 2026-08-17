#!/usr/bin/env python3
"""
Builds tests/fixtures/ntfs_basic.img: a minimal, byte-exact NTFS-format
image, hand-constructed rather than produced with mkntfs — the same
reasoning as the FAT32 fixtures using mtools instead of a kernel mount:
this needs no special tools or privileges and works identically on any
machine, including inside a plain WSL install.

Layout we build:
  - A real NTFS boot sector (bytes_per_sector=512, sectors_per_cluster=1,
    mft_record_size=1024).
  - MFT record 0: $MFT's own record, with a non-resident $DATA attribute
    describing where the rest of the MFT table's records live on disk
    (a single contiguous run of clusters).
  - MFT record 6: $Bitmap, with a RESIDENT $DATA attribute (tiny, since
    our volume only has a handful of clusters) — every bit set to 1
    (allocated) EXCEPT the bit for the test file's data cluster, which we
    leave 0 (free). This is exactly what a real OS delete does: it frees
    the file's clusters in the bitmap at the same time it clears the
    MFT record's in-use flag.
  - MFT record 10: the test file "canary.txt" — built as a LIVE, in-use
    record first (flags = 0x0001), with a resident $FILE_NAME attribute
    and a non-resident $DATA attribute pointing at one real data cluster
    containing the actual file content — and then we flip its in-use bit
    off, simulating exactly the one-byte change a real NTFS delete makes
    to the MFT record itself.

This mirrors, at the byte level, precisely what Windows does on delete:
clear the MFT record's in-use flag, free the bitmap bits, leave everything
else — name, size, data run, and the actual cluster bytes — untouched.
"""
import struct

BYTES_PER_SECTOR = 512
SECTORS_PER_CLUSTER = 1
CLUSTER_SIZE = BYTES_PER_SECTOR * SECTORS_PER_CLUSTER
MFT_RECORD_SIZE = 1024

# Cluster layout for this tiny synthetic volume:
#   clusters 0..31   : the MFT table itself (32 clusters * 512B = 16KB
#                       = 16 records of 1024B each)
#   cluster  32       : canary.txt's file data
#   clusters 33..39   : free padding
MFT_START_LCN = 8          # boot sector + a little padding first
MFT_CLUSTER_COUNT = 32
DATA_LCN = MFT_START_LCN + MFT_CLUSTER_COUNT   # = 40
TOTAL_CLUSTERS = DATA_LCN + 8                  # a few spare free clusters
TOTAL_SECTORS = TOTAL_CLUSTERS * SECTORS_PER_CLUSTER

CANARY_CONTENT = b"This is a canary file for NTFS recovery testing. Bitmap-verified high-confidence recovery.\n"

RECORD_INDEX_MFT = 0
RECORD_INDEX_BITMAP = 6
RECORD_INDEX_CANARY = 10


def build_boot_sector():
    b = bytearray(BYTES_PER_SECTOR)
    b[0:3] = b"\xEB\x52\x90"          # jmp instruction (cosmetic, not parsed by us)
    b[3:11] = b"NTFS    "             # OEM ID — this is what we check for
    struct.pack_into("<H", b, 0x0B, BYTES_PER_SECTOR)
    b[0x0D] = SECTORS_PER_CLUSTER
    struct.pack_into("<Q", b, 0x28, TOTAL_SECTORS)
    struct.pack_into("<Q", b, 0x30, MFT_START_LCN)   # $MFT starting cluster
    # clusters_per_mft_record: negative byte => record size = 2^|value|.
    # 2^10 = 1024, matching MFT_RECORD_SIZE.
    b[0x40] = (-10) & 0xFF
    b[0x1FE] = 0x55
    b[0x1FF] = 0xAA
    return bytes(b)


def encode_data_run(start_lcn, length_clusters):
    """Single-run encoder: length and offset each fit in a couple of
    bytes for our tiny synthetic volume, keeping this simple rather than
    handling every width case (the Rust decoder handles the general case;
    we only need to emit ONE specific, easy-to-reason-about encoding)."""
    length_bytes = length_clusters.to_bytes(2, "little")            # 2-byte length
    offset_bytes = start_lcn.to_bytes(2, "little", signed=True)      # 2-byte signed offset
    header = (len(offset_bytes) << 4) | len(length_bytes)
    return bytes([header]) + length_bytes + offset_bytes + b"\x00"   # + terminator


def build_non_resident_data_attr(runs):
    """runs: list of (start_lcn, length_clusters). Builds a full $DATA
    (0x80) non-resident attribute record, header + run list, correctly
    padded to a multiple of 8 bytes as NTFS expects."""
    run_bytes = b"".join(encode_data_run(lcn, length) for lcn, length in runs)
    header_len = 64  # fixed non-resident header size we use below
    run_offset = header_len
    total_len = run_offset + len(run_bytes)
    # pad to 8-byte alignment
    pad = (-total_len) % 8
    total_len += pad

    total_clusters = sum(length for _, length in runs)
    allocated_size = total_clusters * CLUSTER_SIZE

    attr = bytearray(total_len)
    struct.pack_into("<I", attr, 0, 0x80)          # attribute type = $DATA
    struct.pack_into("<I", attr, 4, total_len)      # attribute length
    attr[8] = 1                                     # non_resident = true
    attr[9] = 0                                      # name_length
    struct.pack_into("<H", attr, 10, 0)             # name_offset
    struct.pack_into("<H", attr, 12, 0)             # flags
    struct.pack_into("<H", attr, 14, 0)             # attribute_id
    struct.pack_into("<Q", attr, 16, 0)             # starting VCN
    struct.pack_into("<Q", attr, 24, total_clusters - 1)  # ending VCN
    struct.pack_into("<H", attr, 32, run_offset)    # data_run_offset — what our parser reads
    struct.pack_into("<H", attr, 34, 0)             # compression unit size
    struct.pack_into("<Q", attr, 40, allocated_size)
    struct.pack_into("<Q", attr, 48, allocated_size)  # real size (fine to match here)
    struct.pack_into("<Q", attr, 56, allocated_size)  # initialized size
    attr[run_offset:run_offset + len(run_bytes)] = run_bytes
    return bytes(attr)


def build_resident_data_attr(content: bytes):
    header_len = 24
    total_len = header_len + len(content)
    pad = (-total_len) % 8
    total_len += pad

    attr = bytearray(total_len)
    struct.pack_into("<I", attr, 0, 0x80)   # $DATA
    struct.pack_into("<I", attr, 4, total_len)
    attr[8] = 0    # resident
    attr[9] = 0
    struct.pack_into("<H", attr, 10, 0)
    struct.pack_into("<H", attr, 12, 0)
    struct.pack_into("<H", attr, 14, 0)
    struct.pack_into("<I", attr, 16, len(content))   # content_length
    struct.pack_into("<H", attr, 20, header_len)      # content_offset
    attr[22] = 0    # indexed flag
    attr[23] = 0    # padding
    attr[header_len:header_len + len(content)] = content
    return bytes(attr)


def build_file_name_attr(name: str, real_size: int):
    name_utf16 = name.encode("utf-16-le")
    content_len = 66 + len(name_utf16)
    content = bytearray(content_len)
    # parent dir ref (offset 0, 8 bytes) — left as 0, not needed for Phase 4
    # four 8-byte timestamps at 8,16,24,32 — left as 0, not needed
    struct.pack_into("<Q", content, 40, real_size)   # allocated size
    struct.pack_into("<Q", content, 48, real_size)   # real size
    struct.pack_into("<I", content, 56, 0x20)        # flags: FILE_ATTRIBUTE_ARCHIVE
    content[64] = len(name)                           # filename length in chars
    content[65] = 1                                    # namespace: Win32
    content[66:66 + len(name_utf16)] = name_utf16

    header_len = 24
    total_len = header_len + len(content)
    pad = (-total_len) % 8
    total_len += pad

    attr = bytearray(total_len)
    struct.pack_into("<I", attr, 0, 0x30)   # $FILE_NAME
    struct.pack_into("<I", attr, 4, total_len)
    attr[8] = 0
    attr[9] = 0
    struct.pack_into("<H", attr, 10, 0)
    struct.pack_into("<H", attr, 12, 0)
    struct.pack_into("<H", attr, 14, 0)
    struct.pack_into("<I", attr, 16, len(content))
    struct.pack_into("<H", attr, 20, header_len)
    attr[22] = 1   # indexed flag (filenames normally are)
    attr[23] = 0
    attr[header_len:header_len + len(content)] = content
    return bytes(attr)


def build_mft_record(attrs: list, in_use: bool, is_directory: bool = False):
    """Wraps a list of pre-built attribute byte blobs into a full MFT
    record, including the fixup (Update Sequence Array) machinery — we
    build this correctly here specifically so the Rust `apply_fixups`
    code has a genuine, correctly-formed record to restore."""
    header_len = 48  # room for the fixed header fields we use
    first_attr_offset = header_len + 8  # +8 for a tiny USA area right after

    body = b"".join(attrs) + struct.pack("<I", 0xFFFFFFFF)  # end marker
    used_size = first_attr_offset + len(body)
    used_size += (-used_size) % 8  # 8-byte align, matches real NTFS behavior

    record = bytearray(MFT_RECORD_SIZE)
    record[0:4] = b"FILE"
    usa_offset = header_len
    usa_count = (MFT_RECORD_SIZE // BYTES_PER_SECTOR) + 1  # USN + one per sector
    struct.pack_into("<H", record, 4, usa_offset)
    struct.pack_into("<H", record, 6, usa_count)
    struct.pack_into("<Q", record, 8, 0)     # $LogFile sequence number
    struct.pack_into("<H", record, 16, 1)    # sequence number
    struct.pack_into("<H", record, 18, 1)    # hard link count
    struct.pack_into("<H", record, 20, first_attr_offset)
    flags = (1 if in_use else 0) | (2 if is_directory else 0)
    struct.pack_into("<H", record, 22, flags)
    struct.pack_into("<I", record, 24, used_size)
    struct.pack_into("<I", record, 28, MFT_RECORD_SIZE)

    record[first_attr_offset:first_attr_offset + len(body)] = body

    # --- Apply real NTFS fixups, matching what apply_fixups() expects to undo ---
    usn = b"\x01\x00"  # arbitrary non-zero USN value
    sector_count = MFT_RECORD_SIZE // BYTES_PER_SECTOR
    usa = bytearray(2 + sector_count * 2)
    usa[0:2] = usn
    for i in range(sector_count):
        sector_end = (i + 1) * BYTES_PER_SECTOR - 2
        # Save the real bytes currently there...
        usa[2 + i * 2: 4 + i * 2] = record[sector_end:sector_end + 2]
        # ...then stamp the USN over them, exactly as real NTFS does on write.
        record[sector_end:sector_end + 2] = usn
    record[usa_offset:usa_offset + len(usa)] = usa

    return bytes(record)


def main():
    out_path = "tests/fixtures/ntfs_basic.img"
    total_size = TOTAL_CLUSTERS * CLUSTER_SIZE
    image = bytearray(total_size)

    # --- Boot sector ---
    image[0:BYTES_PER_SECTOR] = build_boot_sector()

    # --- Build record 10: canary.txt, LIVE first ---
    file_name_attr = build_file_name_attr("canary.txt", len(CANARY_CONTENT))
    data_attr = build_non_resident_data_attr([(DATA_LCN, 1)])
    canary_record_live = build_mft_record([file_name_attr, data_attr], in_use=True)

    # Simulate delete: flip ONLY the in-use bit in the flags field
    # (offset 22, low bit) — exactly the single-byte-level change a real
    # NTFS delete makes to the MFT record.
    canary_record_deleted = bytearray(canary_record_live)
    current_flags = struct.unpack_from("<H", canary_record_deleted, 22)[0]
    struct.pack_into("<H", canary_record_deleted, 22, current_flags & ~0x0001)

    # --- Build record 0: $MFT's own record, describing the MFT table's layout ---
    mft_data_attr = build_non_resident_data_attr([(MFT_START_LCN, MFT_CLUSTER_COUNT)])
    mft_record0 = build_mft_record([mft_data_attr], in_use=True)

    # --- Build record 6: $Bitmap, resident, matching our delete simulation ---
    bitmap_size_bytes = (TOTAL_CLUSTERS + 7) // 8
    bitmap_bytes = bytearray(bitmap_size_bytes)
    # Mark the MFT's own clusters allocated (they genuinely are).
    for lcn in range(MFT_START_LCN, MFT_START_LCN + MFT_CLUSTER_COUNT):
        bitmap_bytes[lcn // 8] |= (1 << (lcn % 8))
    # DATA_LCN is deliberately left as 0 (free) — this is the bit a real
    # OS delete would have cleared when canary.txt was removed.
    bitmap_attr = build_resident_data_attr(bytes(bitmap_bytes))
    bitmap_record = build_mft_record([bitmap_attr], in_use=True)

    # --- Place records into the MFT table region ---
    def place_record(index, record_bytes):
        offset = MFT_START_LCN * CLUSTER_SIZE + index * MFT_RECORD_SIZE
        image[offset:offset + MFT_RECORD_SIZE] = record_bytes

    place_record(RECORD_INDEX_MFT, mft_record0)
    place_record(RECORD_INDEX_BITMAP, bitmap_record)
    place_record(RECORD_INDEX_CANARY, canary_record_deleted)

    # --- Place the actual file content at its real data cluster ---
    data_offset = DATA_LCN * CLUSTER_SIZE
    image[data_offset:data_offset + len(CANARY_CONTENT)] = CANARY_CONTENT

    with open(out_path, "wb") as f:
        f.write(image)

    print(f"Fixture ready: {out_path} ({len(image)} bytes)")
    print(f"  canary.txt: MFT record {RECORD_INDEX_CANARY}, data at LCN {DATA_LCN}, "
          f"{len(CANARY_CONTENT)} bytes, in-use flag cleared, bitmap bit freed.")


if __name__ == "__main__":
    main()
