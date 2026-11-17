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
rm -rf "$tmp"
