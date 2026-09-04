#!/usr/bin/env python3
"""Generate a small numbered PNG avatar without external image libraries."""
import struct
import sys
import zlib


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: make_test_avatar.py NUMBER OUTPUT.png")
    number = int(sys.argv[1])
    output = sys.argv[2]
    size = 256
    pixels = bytearray(size * size * 3)
    hue = (number * 31) % 360

    # Deterministic color bands, enough to distinguish every avatar visually.
    colors = [
        (30 + (hue * 3) % 180, 90 + (hue * 5) % 130, 180 + (hue * 7) % 60),
        (245, 245, 255),
    ]
    for y in range(size):
        for x in range(size):
            dx, dy = x - size // 2, y - size // 2
            inside = dx * dx + dy * dy <= 104 * 104
            color = colors[0] if inside else (8, 21, 47)
            offset = (y * size + x) * 3
            pixels[offset:offset + 3] = bytes(color)

    # 5x7 bitmap digits, scaled for a readable avatar label.
    glyphs = {
        "0": ["11111", "10001", "10011", "10101", "11001", "10001", "11111"],
        "1": ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
        "2": ["11110", "00001", "00001", "01110", "10000", "10000", "11111"],
        "3": ["11110", "00001", "00001", "01110", "00001", "00001", "11110"],
        "4": ["10010", "10010", "10010", "11111", "00010", "00010", "00010"],
        "5": ["11111", "10000", "10000", "11110", "00001", "00001", "11110"],
        "6": ["01110", "10000", "10000", "11110", "10001", "10001", "01110"],
        "7": ["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
        "8": ["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
        "9": ["01110", "10001", "10001", "01111", "00001", "00001", "01110"],
    }
    text = str(number)
    scale = 18
    width = len(text) * 5 * scale + (len(text) - 1) * 3 * scale
    left = (size - width) // 2
    top = (size - 7 * scale) // 2
    for char_index, char in enumerate(text):
        glyph = glyphs[char]
        x0 = left + char_index * (5 + 3) * scale
        for gy, row in enumerate(glyph):
            for gx, bit in enumerate(row):
                if bit != "1":
                    continue
                for yy in range(scale):
                    for xx in range(scale):
                        x, y = x0 + gx * scale + xx, top + gy * scale + yy
                        offset = (y * size + x) * 3
                        pixels[offset:offset + 3] = bytes(colors[1])

    raw = b"".join(b"\x00" + pixels[y * size * 3:(y + 1) * size * 3] for y in range(size))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(output, "wb") as handle:
        handle.write(png)


if __name__ == "__main__":
    main()
