#!/bin/sh
# Désassemble une zone d'un vidage de mémoire de boot_report (--dump-ram).
# Usage : sh tools/disasm_ram.sh PRÉFIXE-main.bin DÉBUT FIN [thumb|arm]
#   DÉBUT, FIN : adresses hexadécimales (ex. 02042780 02042800).
# Nécessite llvm-objcopy, ld.lld et llvm-objdump.
set -eu
bin="$1"; start="$2"; stop="$3"; mode="${4:-thumb}"
base=0x02000000
case "$(basename "$bin")" in *wram7.bin) base=0x03800000 ;; esac
tmp="$(mktemp -d)"
cp "$bin" "$tmp/ram.bin"
(cd "$tmp" && llvm-objcopy -I binary -O elf32-littlearm \
    --rename-section=.data=.text,code,alloc,load ram.bin ram.o)
ld.lld -Ttext="$base" -e "$base" "$tmp/ram.o" -o "$tmp/ram.elf"
triple=thumbv5te-none-eabi
[ "$mode" = arm ] && triple=armv5te-none-eabi
llvm-objdump -d --no-show-raw-insn --triple="$triple" \
    --start-address="0x$start" --stop-address="0x$stop" "$tmp/ram.elf" | tail -n +7
rm -rf "$tmp"
