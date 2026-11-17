#!/bin/sh
# Recompile les programmes de test d'intégration et les valeurs attendues.
# Nécessite clang, ld.lld et llvm-objcopy. Usage : sh tools/build_programs.sh
set -eu
cd "$(dirname "$0")/../tests/programs"
CFLAGS="-ffreestanding -nostdlib -fno-builtin -fno-stack-protector -fomit-frame-pointer -Wall -Wno-unused-function"
for arch in armv4t armv5te; do
  for isa in arm thumb; do
    for opt in O0 O2; do
      name="${arch}_${isa}_${opt}"
      clang --target=${arch}-none-eabi -m${isa} -${opt} $CFLAGS -c suite.c -o /tmp/suite_${name}.o
      clang --target=${arch}-none-eabi -c start.s -o /tmp/start_${arch}.o
      ld.lld -T link.ld /tmp/start_${arch}.o /tmp/suite_${name}.o -o /tmp/${name}.elf
      llvm-objcopy -O binary /tmp/${name}.elf ${name}.bin
      echo "${name}.bin : $(wc -c < ${name}.bin) octets"
    done
  done
done
cc -O1 -DHOST -o /tmp/suite_host suite.c
/tmp/suite_host > expected.rs
echo "expected.rs généré"
