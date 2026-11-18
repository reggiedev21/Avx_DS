#!/bin/sh
# Compile les ROMs de test maison (tests/roms/*). Nécessite clang, ld.lld,
# llvm-objcopy et python3. Usage : sh tools/build_test_roms.sh
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
CFLAGS="-ffreestanding -nostdlib -fno-builtin -fno-stack-protector -O2 -Wall -Wextra"
src="$root/tests/roms/boot_test"
# ARM9 : ARMv5TE, C en Thumb, chargé en Main RAM.
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/arm9.o"
clang --target=armv5te-none-eabi -c "$src/crt0.s" -o "$tmp/crt0_9.o"
ld.lld -T "$src/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/arm9.o" -o "$tmp/arm9.elf"
llvm-objcopy -O binary "$tmp/arm9.elf" "$tmp/arm9.bin"
# ARM7 : ARMv4T, en ARM, chargé en WRAM (0x037F8000).
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/arm7.o"
clang --target=armv4t-none-eabi -c "$src/crt0.s" -o "$tmp/crt0_7.o"
ld.lld -T "$src/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/arm7.o" -o "$tmp/arm7.elf"
llvm-objcopy -O binary "$tmp/arm7.elf" "$tmp/arm7.bin"
python3 "$root/tools/make_nds.py" "$root/tests/roms/boot_test.nds" "$tmp/arm9.bin" 0x02000000 "$tmp/arm7.bin" 0x037F8000

# ROM de démonstration des sprites, fenêtres et effets.
src="$root/tests/roms/obj_test"
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/obj9.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/obj9.o" -o "$tmp/obj9.elf"
llvm-objcopy -O binary "$tmp/obj9.elf" "$tmp/obj9.bin"
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/obj7.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/obj7.o" -o "$tmp/obj7.elf"
llvm-objcopy -O binary "$tmp/obj7.elf" "$tmp/obj7.bin"
python3 "$root/tools/make_nds.py" "$root/tests/roms/obj_test.nds" "$tmp/obj9.bin" 0x02000000 "$tmp/obj7.bin" 0x037F8000 "AVXDS OBJ" "AVXO"
# ROM de test des entrées-sorties (cartouche, SPI, horloge), complétée à
# 64 Ko avec un motif connu à partir de 0x8000 pour les lectures B7.
src="$root/tests/roms/io_test"
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/io9.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/io9.o" -o "$tmp/io9.elf"
llvm-objcopy -O binary "$tmp/io9.elf" "$tmp/io9.bin"
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/io7.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/io7.o" -o "$tmp/io7.elf"
llvm-objcopy -O binary "$tmp/io7.elf" "$tmp/io7.bin"
out="$root/tests/roms/io_test.nds"
python3 "$root/tools/make_nds.py" "$out" "$tmp/io9.bin" 0x02000000 "$tmp/io7.bin" 0x037F8000 "AVXDS IO" "AVXI"
python3 - "$out" <<'PY'
import sys
p = sys.argv[1]
rom = bytearray(open(p, "rb").read())
assert len(rom) <= 0x8000
rom += bytes(0x8000 - len(rom))
rom += bytes((i * 7 + (i >> 8)) & 0xFF for i in range(0x8000))
open(p, "wb").write(rom)
PY
# ROM de test de la sauvegarde (EEPROM 8 Ko, 0,5 Ko, FLASH ; mode choisi par le test).
src="$root/tests/roms/save_test"
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/sv9.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/sv9.o" -o "$tmp/sv9.elf"
llvm-objcopy -O binary "$tmp/sv9.elf" "$tmp/sv9.bin"
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/sv7.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/sv7.o" -o "$tmp/sv7.elf"
llvm-objcopy -O binary "$tmp/sv7.elf" "$tmp/sv7.bin"
python3 "$root/tools/make_nds.py" "$root/tests/roms/save_test.nds" "$tmp/sv9.bin" 0x02000000 "$tmp/sv7.bin" 0x037F8000 "AVXDS SAVE" "AVXS"
# ROM de test de la file de commandes 3D.
src="$root/tests/roms/gx_test"
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/gx9.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/gx9.o" -o "$tmp/gx9.elf"
llvm-objcopy -O binary "$tmp/gx9.elf" "$tmp/gx9.bin"
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/gx7.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/gx7.o" -o "$tmp/gx7.elf"
llvm-objcopy -O binary "$tmp/gx7.elf" "$tmp/gx7.bin"
python3 "$root/tools/make_nds.py" "$root/tests/roms/gx_test.nds" "$tmp/gx9.bin" 0x02000000 "$tmp/gx7.bin" 0x037F8000 "AVXDS GX" "AVXG"
# ROM de test du son (programme ARM7, l'ARM9 dort).
src="$root/tests/roms/sound_test"
clang --target=armv5te-none-eabi -mthumb $CFLAGS -c "$src/arm9.c" -o "$tmp/sn9.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x02000000 --defsym=MAIN=main9 "$tmp/crt0_9.o" "$tmp/sn9.o" -o "$tmp/sn9.elf"
llvm-objcopy -O binary "$tmp/sn9.elf" "$tmp/sn9.bin"
clang --target=armv4t-none-eabi -marm $CFLAGS -c "$src/arm7.c" -o "$tmp/sn7.o"
ld.lld -T "$root/tests/roms/boot_test/link.ld" --defsym=LOAD_ADDR=0x037F8000 --defsym=MAIN=main7 "$tmp/crt0_7.o" "$tmp/sn7.o" -o "$tmp/sn7.elf"
llvm-objcopy -O binary "$tmp/sn7.elf" "$tmp/sn7.bin"
python3 "$root/tools/make_nds.py" "$root/tests/roms/sound_test.nds" "$tmp/sn9.bin" 0x02000000 "$tmp/sn7.bin" 0x037F8000 "AVXDS SOUND" "AVXA"
rm -rf "$tmp"
