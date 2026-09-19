#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Tri-way deep comparison: INPUT (NRMM-Rust-test) vs BASELINE (NRMM-test) vs RUNTIME (update_mod_data_runtime).
Classifies each file path to separate genuine Rust gaps from dataset drift.
Exclusions: desktop.ini (whitelist), *.ico (ignore), *_managed_backup / *.bak / *.baknamespace (backup artifacts), d3dx.ini_managed_backup.
"""
import hashlib, os, sys

BASE = r"d:/PC/TestProjects/XXMI-NRMM/src-tauri/tests"
INPUT = os.path.join(BASE, "NRMM-Rust-test")
BASELINE = os.path.join(BASE, "NRMM-test")
RUNTIME = os.path.join(BASE, "update_mod_data_runtime")

EXCLUDE_NAMES = {"desktop.ini", "d3dx.ini_managed_backup"}
EXCLUDE_SUFFIX = (".ico", "_managed_backup", ".bak", ".baknamespace")

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()

def walk(root):
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        for fn in filenames:
            rel = os.path.relpath(os.path.join(dirpath, fn), root)
            rel = rel.replace("\\", "/")
            low = fn.lower()
            if low in EXCLUDE_NAMES:
                continue
            if low.endswith(EXCLUDE_SUFFIX):
                continue
            out[rel] = os.path.join(dirpath, fn)
    return out

def main():
    I = walk(INPUT)
    B = walk(BASELINE)
    R = walk(RUNTIME)
    si, sb, sr = set(I), set(B), set(R)

    print(f"=== COUNT (excl. desktop.ini/ico/backup) ===")
    print(f"INPUT(files)={len(si)}  BASELINE={len(sb)}  RUNTIME={len(sr)}")

    only_B = sorted(sb - sr - si)   # baseline has, runtime lacks, input lacks -> dart-generated or drift
    only_B_in_I = sorted((sb - sr) & si)  # baseline has, runtime lacks, BUT input has -> rust gap
    only_R = sorted(sr - sb)        # runtime extra vs baseline
    only_I = sorted(si - sb - sr)   # input extra (untouched by either?)
    common_BR = sorted(sb & sr)
    common_BI = sorted(sb & si)

    print(f"\n=== B only (baseline has, runtime lacks) total={len(sb-sr)} ===")
    print(f"  -> of which also in INPUT (Rust gap candidates): {len(only_B_in_I)}")
    for p in only_B_in_I:
        print(f"  GAP   {p}")
    print(f"  -> of which NOT in INPUT (dart-gen/drift): {len(only_B)}")
    for p in only_B:
        print(f"  DRIFT {p}")

    print(f"\n=== R only (runtime extra vs baseline) total={len(only_R)} ===")
    for p in only_R:
        print(f"  EXTRA {p}")

    print(f"\n=== I only (input has, neither baseline nor runtime) total={len(only_I)} ===")
    for p in only_I[:200]:
        print(f"  INP   {p}")
    if len(only_I) > 200:
        print(f"  ... and {len(only_I)-200} more")

    # content diff among common BR
    print(f"\n=== CONTENT DIFF among B&R common ({len(common_BR)}) ===")
    ndiff = 0
    for p in common_BR:
        hb = sha256(B[p]); hr = sha256(R[p])
        if hb != hr:
            ndiff += 1
            print(f"  DIFF  {p}")
    print(f"  total content-differing common files: {ndiff}")

    # among B&I common, any content diff? (dart transformation check)
    print(f"\n=== B&I common ({len(common_BI)}) content diff (Dart transform) ===")
    nd = 0
    for p in common_BI:
        if sha256(B[p]) != sha256(I[p]):
            nd += 1
            if nd <= 60:
                print(f"  TRANSFORM {p}")
    print(f"  total Dart-transformed (B!=I) common files: {nd}")

if __name__ == "__main__":
    main()
