"""Independent FAT chain/file extraction verification for the generated disk."""
import hashlib
from pathlib import Path
import struct
import unittest
from make_disk import ROOT, TARGETS, make_image


def extract(image):
    bps, spc, reserved, fats, entries, sectors = struct.unpack_from('<HBHBHH', image, 11)
    fatsz = struct.unpack_from('<H', image, 22)[0]
    assert bps == 512 and spc == 1 and image[510:512] == b'\x55\xaa'
    assert len(image) == sectors * bps
    fat = image[reserved*bps:(reserved+fatsz)*bps]
    assert fat == image[(reserved+fatsz)*bps:(reserved+2*fatsz)*bps]
    root = (reserved+fats*fatsz)*bps
    data = root + entries*32
    result = {}
    for pos in range(root, data, 32):
        if image[pos] == 0:
            break
        if image[pos+11] & 8:
            continue
        name = image[pos:pos+8].decode().rstrip() + '.' + image[pos+8:pos+11].decode().rstrip()
        name = name.rstrip('.')
        cluster, size = struct.unpack_from('<HI', image, pos+26)
        payload = bytearray()
        seen = set()
        while cluster < 0xfff8:
            assert 2 <= cluster < len(fat)//2 and cluster not in seen
            seen.add(cluster)
            off = data + (cluster-2)*bps
            assert off+bps <= len(image)
            payload += image[off:off+bps]
            cluster = struct.unpack_from('<H', fat, cluster*2)[0]
        assert len(payload) >= size and len(payload)-size < bps
        result[name] = payload[:size]
    return result


def windows_fat_bits(image):
    # Microsoft's fastfat FatNumberOfClusters/FatIndexBitSize (fat.h):
    # https://github.com/microsoft/Windows-driver-samples/blob/main/filesys/fastfat/fat.h
    bps, spc, reserved, fats, entries, sectors = struct.unpack_from('<HBHBHH', image, 11)
    fatsz = struct.unpack_from('<H', image, 22)[0]
    clusters = (sectors - reserved - fats*fatsz - entries*32//bps)//spc
    return 12 if clusters < 4087 else 16


class DiskTests(unittest.TestCase):
    def test_windows_recognizes_fat16(self):
        self.assertEqual(windows_fat_bits(make_image([('SMALL.TXT', b'hello')])), 16)
        image = (ROOT/'output/examples/rp2350/host-tools.img').read_bytes()
        self.assertEqual(windows_fat_bits(image), 16)

    def test_cluster_boundaries(self):
        files = [('ONE.BIN', b'a'), ('EXACT.BIN', b'b'*512), ('MULTI.BIN', bytes(range(256))*123)]
        self.assertEqual(extract(make_image(files)), dict(files))

    def test_packaged_executables_and_hashes(self):
        files = extract((ROOT/'output/examples/rp2350/host-tools.img').read_bytes())
        self.assertEqual(set(files), {'README.TXT', 'SHA256.TXT', 'HOST.TXT', 'PCMON.RUL', *TARGETS})
        for line in files['SHA256.TXT'].decode().splitlines():
            sha, name = line.split()
            self.assertEqual(hashlib.sha256(files[name]).hexdigest(), sha)
        for name, (target, exe) in TARGETS.items():
            self.assertEqual(files[name], (ROOT/'output/host'/target/exe).read_bytes())

if __name__ == '__main__':
    unittest.main()
