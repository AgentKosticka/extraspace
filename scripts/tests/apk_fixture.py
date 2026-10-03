"""Small binary-XML APK fixtures, not installable Android applications."""
import struct
import zipfile

def apk(path, version, package="io.github.tymonoman.extraspace", utf8=True):
    strings = ["manifest", "package", package, "versionCode"]
    offsets = []
    payload = b""
    for s in strings:
        offsets.append(len(payload))
        if utf8:
            encoded = s.encode()
            payload += bytes([len(s), len(encoded)]) + encoded + b"\0"
        else:
            encoded = s.encode("utf-16le")
            payload += struct.pack("<H", len(s)) + encoded + b"\0\0"
    payload += b"\0" * (-len(payload) % 4)
    start = 28 + 4 * len(strings)
    pool = struct.pack("<HHIIIIII", 1, 28, start + len(payload), len(strings), 0, 0x100 if utf8 else 0, start, 0)
    pool += struct.pack("<" + "I" * len(strings), *offsets) + payload
    attrs = struct.pack("<IIIHBBI", 0xffffffff, 1, 2, 8, 0, 3, 2)
    attrs += struct.pack("<IIIHBBI", 0xffffffff, 3, 0xffffffff, 8, 0, 0x10, version)
    element = struct.pack("<HHIII", 0x102, 16, 36 + len(attrs), 1, 0xffffffff)
    element += struct.pack("<IIHHHHHH", 0xffffffff, 0, 20, 20, 2, 0, 0, 0) + attrs
    xml = struct.pack("<HHI", 3, 8, 8 + len(pool) + len(element)) + pool + element
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("AndroidManifest.xml", xml)
