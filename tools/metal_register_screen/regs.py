"""Print the register counts of the GPU slice of a serialized MTLBinaryArchive.

usage: python3 -I regs.py <pipeline.metallib>

The archive is a fat binary. Its AMD GPU slice (CPU type 0x01000014, RDNA
gfx10 on the Mac target) is a Mach-O. Its `__TEXT,__fragment` section holds a
packed function: after the name `air.packedFragmentFunction` come a u32 at +4
holding SGPRs and a u32 at +8 holding VGPRs. That fragment lookup is verified.
The vertex entry (`__TEXT,__vertex`, `air.new.packedVertexFunction`, same
offsets) is unverified and reports `not found` on the screened pipeline.
Reverse-engineered, not a documented format: if Apple changes it, re-derive the offsets before trusting a
number. Standard library only.
"""

import struct
import sys

AMD_GPU_CPU_TYPE = 0x01000014
LC_SEGMENT_64 = 0x19
STAGES = (
    ("vertex", "__vertex", b"air.new.packedVertexFunction\0"),
    ("fragment", "__fragment", b"air.packedFragmentFunction\0"),
)


def slices(data):
    """(cpu type, bytes) for each slice of a big-endian fat binary."""
    _magic, count = struct.unpack(">II", data[:8])
    out = []
    for i in range(count):
        cpu_type, _sub, offset, size, _align = struct.unpack(
            ">iiIII", data[8 + 20 * i : 28 + 20 * i]
        )
        out.append((cpu_type, data[offset : offset + size]))
    return out


def sections(macho):
    """Section name -> bytes for every section of a 64-bit Mach-O."""
    command_count = struct.unpack("<I", macho[16:20])[0]
    offset = 32
    out = {}
    for _ in range(command_count):
        command, size = struct.unpack("<II", macho[offset : offset + 8])
        if command == LC_SEGMENT_64:
            section_count = struct.unpack("<I", macho[offset + 64 : offset + 68])[0]
            at = offset + 72
            for _ in range(section_count):
                header = macho[at : at + 80]
                name = header[:16].rstrip(b"\0").decode()
                _addr, length, file_offset = struct.unpack("<QQI", header[32:52])
                out[name] = macho[file_offset : file_offset + length]
                at += 80
        offset += size
    return out


def main(path):
    with open(path, "rb") as f:
        data = f.read()
    found = False
    for cpu_type, macho in slices(data):
        if cpu_type != AMD_GPU_CPU_TYPE:
            continue
        found = True
        found_sections = sections(macho)
        for stage, section, tag in STAGES:
            body = found_sections.get(section)
            at = body.find(tag) if body else -1
            if at < 0:
                print(f"{stage}: not found")
                continue
            at += len(tag)
            sgprs, vgprs = struct.unpack("<II", body[at + 4 : at + 12])
            print(f"{stage}: sgprs={sgprs} vgprs={vgprs}")
    if not found:
        sys.exit("no AMD GPU slice in the archive (is the discrete GPU an RDNA part?)")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
