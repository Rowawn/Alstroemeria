#!/usr/bin/env python3
"""Converts libhangul's data/hanja/hanja.txt (BSD-3-Clause, see extensions/ko-hanja/DATA-LICENSE) into
extensions/ko-hanja/hanja.tsv (`hangul<TAB>hanja<TAB>meaning`). Usage: build-hanja-tsv.py path/to/hanja.txt"""
import pathlib, sys

seen, words, chars = set(), [], {}
for line in open(sys.argv[1], encoding="utf8"):
    if line.startswith("#") or not line.strip():
        continue
    p = line.rstrip("\n").split(":")
    if len(p) < 2:
        continue
    word, hanja, meaning = p[0], p[1], ":".join(p[2:])
    if not word or not hanja or (word, hanja) in seen or not all("\uac00" <= c <= "\ud7a3" for c in word):
        continue
    seen.add((word, hanja))
    if len(word) == 1 and len(hanja) == 1:
        if meaning and hanja not in chars:
            chars[hanja] = meaning
    else:
        words.append((word, hanja, meaning))
words.sort(key=lambda r: r[0])  # stable: upstream order kept within a word; code-unit order, as JS compares
root = pathlib.Path(__file__).resolve().parent.parent / "extensions/ko-hanja"
(root / "hanja.tsv").write_text("".join(f"{w}\t{h}\t{m}\n" for w, h, m in words), encoding="utf8")
(root / "hanja-chars.tsv").write_text("".join(f"{h}\t{m}\n" for h, m in chars.items()), encoding="utf8")
print(len(words), "words,", len(chars), "characters")
