#!/usr/bin/env python3
"""Check APK manifest identity/version using only Python's standard library."""
import struct
import sys
import zipfile

PACKAGE = "io.github.tymonoman.extraspace"
MAX_MANIFEST = 4 * 1024 * 1024

def manifest_info(data):
    if len(data) < 8 or struct.unpack_from("<H", data)[0] != 3:
        raise ValueError("APK manifest is not Android binary XML")
    total = struct.unpack_from("<I", data, 4)[0]
    if total != len(data):
        raise ValueError("Truncated or extended Android manifest")
    strings = []
    def u32(offset):
        return struct.unpack_from("<I", data, offset)[0]
    def string(index):
        if index >= len(strings):
            raise ValueError("Invalid manifest string index")
        return strings[index]
    pos = struct.unpack_from("<H", data, 2)[0]
    while pos < total:
        kind, header, size = struct.unpack_from("<HHI", data, pos)
        if header < 8 or size < header or pos + size > total:
            raise ValueError("Invalid Android manifest chunk")
        end = pos + size
        if kind == 1:
            if header < 28:
                raise ValueError("Invalid manifest string pool")
            count, flags, start = u32(pos + 8), u32(pos + 16), u32(pos + 20)
            if count > (size - header) // 4:
                raise ValueError("Oversized manifest string pool")
            utf8 = bool(flags & 0x100)
            def length(at):
                if utf8:
                    first = data[at]; at += 1
                    if first & 0x80:
                        first = ((first & 0x7f) << 8) | data[at]; at += 1
                else:
                    first = struct.unpack_from("<H", data, at)[0]; at += 2
                    if first & 0x8000:
                        first = ((first & 0x7fff) << 16) | struct.unpack_from("<H", data, at)[0]; at += 2
                return first, at
            for i in range(count):
                at = pos + start + u32(pos + header + i * 4)
                if not pos + header <= at < end:
                    raise ValueError("Invalid manifest string offset")
                n, at = length(at)
                if utf8:
                    n, at = length(at)
                else:
                    n *= 2
                if at + n + (1 if utf8 else 2) > end:
                    raise ValueError("Truncated manifest string")
                strings.append(data[at:at + n].decode("utf-8" if utf8 else "utf-16le"))
        elif kind == 0x102:
            if header < 16 or size < header + 20:
                raise ValueError("Invalid manifest element")
            ext = pos + header
            if string(u32(ext + 4)) == "manifest":
                attr_start, attr_size, count = struct.unpack_from("<HHH", data, ext + 8)
                if attr_size < 20 or ext + attr_start + count * attr_size > end:
                    raise ValueError("Invalid manifest attributes")
                values = {}
                for i in range(count):
                    at = ext + attr_start + i * attr_size
                    name = string(u32(at + 4))
                    value_type = data[at + 15]
                    value = u32(at + 16)
                    values[name] = string(value) if value_type == 3 else value if value_type in (0x10, 0x11) else None
                package, version = values.get("package"), values.get("versionCode")
                if not isinstance(package, str) or not isinstance(version, int) or version <= 0:
                    raise ValueError("APK lacks a valid package/versionCode")
                if values.get("versionCodeMajor", 0) not in (0, None):
                    raise ValueError("Unsupported APK major version")
                return package, version
        pos = end
    raise ValueError("APK lacks its manifest element")

def check(path, expected):
    with zipfile.ZipFile(path) as apk:
        manifests = [entry for entry in apk.infolist() if entry.filename == "AndroidManifest.xml"]
        if len(manifests) != 1 or manifests[0].file_size > MAX_MANIFEST:
            raise ValueError("APK must contain one bounded AndroidManifest.xml")
        package, version = manifest_info(apk.read(manifests[0]))
    if package != PACKAGE:
        raise ValueError(f"Wrong APK package: {package}; expected {PACKAGE}")
    if version != expected:
        raise ValueError(f"APK companion version is {version}, but this host requires {expected}. Build a matching APK or use the published installer.")
    return package, version

if __name__ == "__main__":
    try:
        if len(sys.argv) != 3:
            raise ValueError("Usage: check-apk.py APK EXPECTED_VERSION")
        check(sys.argv[1], int(sys.argv[2]))
    except (ValueError, OSError, zipfile.BadZipFile, struct.error, IndexError, UnicodeError, RuntimeError) as error:
        print(f"Companion APK check failed: {error}", file=sys.stderr)
        sys.exit(1)
