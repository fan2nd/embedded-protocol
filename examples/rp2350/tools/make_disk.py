#!/usr/bin/env python3
"""Build a deterministic FAT16 superfloppy containing actual native host tools."""
import argparse
import hashlib
import math
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[3]
TARGETS = {
    'WIN64.EXE': ('x86_64-pc-windows-gnu', 'pc-monitor-host.exe'),
    'LINUX64': ('x86_64-unknown-linux-musl', 'pc-monitor-host'),
    'MACARM': ('aarch64-apple-darwin', 'pc-monitor-host'),
}
README = b'''PCMON HOST TOOLS - read-only USB disk\r
Copy the matching executable to your computer before running. No extraction needed.\r
WIN64: Windows x64. LINUX64: Linux x64 (static musl).\r
MACARM: Apple Silicon (ARM64).\r
Windows: WIN64.EXE --interval 0.25\r
macOS: chmod +x MACARM; ./MACARM --interval 0.25\r
Linux: chmod +x LINUX64; ./LINUX64 --interval 0.25\r
Linux serial permissions: see HOST.TXT. PCMON.RUL is the example udev rule;\r
install it as 70-pc-monitor.rules if needed.\r
No automatic execution, installation or background service.\r
The USB serial interface remains available while this disk is mounted.\r
KEY1 switches dual 240-degree gauges / last 60 seconds of history.\r
'''

def executable(target, name, host_root):
    exe = host_root / target / name
    if not exe.is_file():
        raise SystemExit(f'Missing native host: {exe}; build all three targets first')
    binary = exe.read_bytes()
    magic = b'MZ' if 'windows' in target else b'\x7fELF' if 'linux' in target else b'\xcf\xfa\xed\xfe'
    if not binary.startswith(magic):
        raise SystemExit(f'Wrong executable format: {exe}')
    return binary

def make_image(files):
    used = sum(math.ceil(len(data)/512) for _, data in files)
    # Windows fastfat treats fewer than 4087 data clusters as FAT12,
    # despite the FAT specification's 4085 threshold. Keep clear of both.
    # A FAT16 table misread as FAT12 lists names but corrupts file chains.
    clusters = max(4128, used + 16)  # FAT16, one 512-byte sector per cluster
    fatsz = math.ceil((clusters + 2)*2/512)
    root_sectors = 4  # 64 short directory entries
    first_data = 1 + 2*fatsz + root_sectors
    image = bytearray((first_data + clusters)*512)
    image[:11] = b'\xeb\x3c\x90PCMON1  '
    struct.pack_into('<HBHBHHBHHHII', image, 11, 512, 1, 1, 2, 64, len(image)//512, 0xf8, fatsz, 32, 64, 0, 0)
    image[36:62] = b'\x80\0\x29' + struct.pack('<I', 0x50434d32) + b'PCMON HOST ' + b'FAT16   '
    image[510:512] = b'\x55\xaa'
    fat = bytearray(fatsz*512)
    struct.pack_into('<HH', fat, 0, 0xfff8, 0xffff)
    root = (1 + 2*fatsz)*512
    image[root:root+11] = b'PCMON HOST '
    image[root+11] = 8
    cluster = 2
    for i, (name, data) in enumerate(files, 1):
        base, _, ext = name.partition('.')
        entry = root + i*32
        image[entry:entry+11] = (base.ljust(8) + ext.ljust(3)).encode('ascii')
        image[entry+11] = 0x21  # archive and read-only
        struct.pack_into('<HH', image, entry+22, 0, (46 << 9) | (1 << 5) | 1)
        struct.pack_into('<HI', image, entry+26, cluster, len(data))
        count = math.ceil(len(data)/512)
        offset = (first_data + cluster - 2)*512
        image[offset:offset+len(data)] = data
        for j in range(count):
            struct.pack_into('<H', fat, (cluster+j)*2, 0xffff if j == count-1 else cluster+j+1)
        cluster += count
    image[512:512+len(fat)] = fat
    image[512+len(fat):512+2*len(fat)] = fat
    return image

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--host-root', type=Path, default=ROOT/'output/host')
    p.add_argument('--output', type=Path, default=ROOT/'output/examples/rp2350/host-tools.img')
    args = p.parse_args()
    files = [('README.TXT', README)]
    files.extend((name, executable(target, exe, args.host_root)) for name, (target, exe) in TARGETS.items())
    files.append(('HOST.TXT', (ROOT/'apps/host/README.md').read_bytes()))
    files.append(('PCMON.RUL', (ROOT/'packaging/linux/70-pc-monitor.rules').read_bytes()))
    sums = ''.join(f'{hashlib.sha256(data).hexdigest()}  {name}\r\n' for name, data in files).encode()
    files.append(('SHA256.TXT', sums))
    image = make_image(files)
    if len(image) > 3*1024*1024:
        raise SystemExit('Disk exceeds 3 MiB budget; strip host executables before packaging')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(image)
    for name, data in files:
        print(f'{name}: {len(data):,} bytes')
    print(f'FAT16 image: {len(image):,} bytes -> {args.output}')
if __name__ == '__main__':
    main()
