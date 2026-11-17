#!/usr/bin/env python3
"""Assemble une ROM .nds à partir de deux binaires bruts (ARM9 et ARM7).

En-tête minimal pour le direct boot : titre, code, offsets, adresses,
tailles et CRC de l'en-tête. Pas de logo Nintendo (inutile en direct boot).

Usage : make_nds.py sortie.nds arm9.bin ADDR9 arm7.bin ADDR7 [TITRE] [CODE]
"""
import struct
import sys


def crc16(data, crc=0xFFFF):
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def align(n, a=0x200):
    return (n + a - 1) // a * a


def main():
    out, a9, addr9, a7, addr7 = sys.argv[1:6]
    title = (sys.argv[6] if len(sys.argv) > 6 else "AVXDS TEST").encode()[:12]
    code = (sys.argv[7] if len(sys.argv) > 7 else "AVXT").encode()[:4]
    arm9 = open(a9, "rb").read()
    arm7 = open(a7, "rb").read()
    addr9, addr7 = int(addr9, 0), int(addr7, 0)
    off9 = 0x200
    off7 = align(off9 + len(arm9))
    end = align(off7 + len(arm7))
    size = 128 << 10
    capacity = 0
    while size < end:
        size <<= 1
        capacity += 1
    h = bytearray(0x200)
    h[0x00:0x0C] = title.ljust(12, b"\0")
    h[0x0C:0x10] = code.ljust(4, b"\0")
    h[0x10:0x12] = b"00"
    h[0x14] = capacity
    struct.pack_into("<IIII", h, 0x20, off9, addr9, addr9, len(arm9))
    struct.pack_into("<IIII", h, 0x30, off7, addr7, addr7, len(arm7))
    struct.pack_into("<I", h, 0x80, end)       # taille utilisée de la ROM
    struct.pack_into("<I", h, 0x84, 0x200)     # taille de l'en-tête
    struct.pack_into("<H", h, 0x15E, crc16(h[:0x15E]))
    rom = bytearray(end)
    rom[:0x200] = h
    rom[off9:off9 + len(arm9)] = arm9
    rom[off7:off7 + len(arm7)] = arm7
    open(out, "wb").write(rom)
    print(f"{out} : {end} octets (ARM9 {len(arm9)} o en {addr9:#x}, ARM7 {len(arm7)} o en {addr7:#x})")


if __name__ == "__main__":
    main()
