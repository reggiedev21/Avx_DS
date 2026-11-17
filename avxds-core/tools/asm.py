#!/usr/bin/env python3
"""Réassemble les blocs de code des tests unitaires.

Dans un fichier Rust, un bloc ressemble à :

    // asm: arm            (ou : thumb, arm-v4, thumb-v4)
    &[
        0x0, // mov r0, pc
        0x0, // loop: subs r1, r1, #1
        0x0, // bne loop
    ]

Chaque ligne `0x…, // <instruction>` est réassemblée par llvm-mc et la
valeur hexadécimale est remplacée. Un préfixe `label:` est autorisé. En
Thumb, un `bl` occupe deux demi-mots sur la même ligne.

Usage : python3 tools/asm.py src/cpu/tests.rs   (réécrit le fichier)
Nécessite llvm-mc, ld.lld et llvm-objdump dans le PATH (paquet LLVM).
"""
import re
import subprocess
import sys
import tempfile
import os

LINE = re.compile(r'^(\s*)((?:0x[0-9A-Fa-f]+,\s*)+)//\s*(.*)$')
HEADER = re.compile(r'//\s*asm:\s*(arm|thumb)(-v4)?\s*$')


def assemble(kind, v4, lines):
    triple = 'armv4t-none-eabi' if v4 else 'armv5te-none-eabi'
    src = ['.syntax unified', '.arm' if kind == 'arm' else '.thumb']
    for l in lines:
        src.append(l)
    with tempfile.TemporaryDirectory() as d:
        s = os.path.join(d, 'a.s')
        o = os.path.join(d, 'a.o')
        open(s, 'w').write('\n'.join(src) + '\n')
        e = os.path.join(d, 'a.elf')
        subprocess.run(['llvm-mc', '-triple=' + triple, '-filetype=obj', s, '-o', o], check=True)
        # Édition de liens à l'adresse 0 pour résoudre les BL vers des labels.
        subprocess.run(['ld.lld', '-Ttext=0', '-e', '0', o, '-o', e], check=True)
        out = subprocess.run(['llvm-objdump', '-d', '--triple=' + triple, e],
                             check=True, capture_output=True, text=True).stdout
    words = []
    for l in out.splitlines():
        m = re.match(r'^\s*([0-9a-f]+):\s(.*)$', l)
        if not m or '\t' not in m.group(2):
            continue
        chunks = m.group(2).split('\t')[0].split()
        if not chunks or not all(re.fullmatch(r'[0-9a-f]+', c) for c in chunks):
            continue
        if all(len(c) == 2 for c in chunks):
            # Données (.word, .short) affichées octet par octet (petit-boutiste)
            w = int(''.join(reversed(chunks)), 16)
            if kind == 'thumb':
                words.append([(w >> (16 * k)) & 0xFFFF for k in range(len(chunks) // 2)])
            else:
                words.append([w])
        else:
            words.append([int(c, 16) for c in chunks])
    return words


def instr_of(comment):
    c = comment.strip()
    m = re.match(r'^([A-Za-z_.][\w.]*):\s*(.*)$', c)
    return c, (m.group(2) if m else c)


def process(path):
    text = open(path).read().split('\n')
    out = []
    i = 0
    while i < len(text):
        line = text[i]
        h = HEADER.search(line)
        out.append(line)
        i += 1
        if not h:
            continue
        kind, v4 = h.group(1), bool(h.group(2))
        block = []
        j = i
        while j < len(text) and not text[j].lstrip().startswith(']'):
            m = LINE.match(text[j])
            if m:
                block.append((j, m.group(1), m.group(3)))
            j += 1
        src = []
        for _, _, comment in block:
            full, ins = instr_of(comment)
            label = full[:len(full) - len(ins)].strip()
            if label:
                src.append(label)
            src.append(ins)
        words = assemble(kind, v4, src)
        if len(words) != len(block):
            sys.exit(f'{path}:{i}: {len(block)} lignes mais {len(words)} instructions')
        fmt = '0x{:08X}' if kind == 'arm' else '0x{:04X}'
        repl = {}
        for (lineno, indent, comment), w in zip(block, words):
            repl[lineno] = indent + ''.join(fmt.format(x) + ', ' for x in w) + '// ' + comment
        for k in range(i, j):
            out.append(repl.get(k, text[k]))
        i = j
    open(path, 'w').write('\n'.join(out))


if __name__ == '__main__':
    for p in sys.argv[1:]:
        process(p)
