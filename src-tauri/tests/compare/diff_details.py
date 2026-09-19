#!/usr/bin/env python3
# -*- coding: utf-8 -*-
import difflib, os
B = r"d:/PC/TestProjects/XXMI-NRMM/src-tauri/tests/NRMM-test"
R = r"d:/PC/TestProjects/XXMI-NRMM/src-tauri/tests/update_mod_data_runtime"
files = [
    "Mods/_MANAGED_/group_1/group_1.ini",
    "Mods/_MANAGED_/group_1/selectedindex",
    "Mods/_MANAGED_/manager_group.ini",
    "Mods/_MANAGED_/nrmm_include.ini",
    "Mods/_MANAGED_/nrmm_keypress.txt",
    "Mods/_MANAGED_/group_1/女主-异界联动 4.3/Stelle.ini",
    "Mods/_MANAGED_/group_1/开拓者·星-云璃表情/女主-云璃表情/Stelle_Cuter_Face/Config.ini",
    "Mods/_MANAGED_/group_1/开拓者·星-哈迪斯泳装无裙子/女主-哈迪斯泳装无裙子/NvzhuMod/Nvzhu.ini",
]
for f in files:
    bp = os.path.join(B, f); rp = os.path.join(R, f)
    if not os.path.exists(bp) or not os.path.exists(rp):
        print(f"\n##### SKIP (missing) {f}\n  base={os.path.exists(bp)} run={os.path.exists(rp)}")
        continue
    with open(bp, encoding="utf-8", errors="replace") as fh: bl = fh.readlines()
    with open(rp, encoding="utf-8", errors="replace") as fh: rl = fh.readlines()
    print(f"\n##### {f}  (base_lines={len(bl)} run_lines={len(rl)})")
    diff = list(difflib.unified_diff(bl, rl, fromfile="BASELINE", tofile="RUNTIME", lineterm=""))
    if not diff:
        print("  (identical)")
    else:
        for ln in diff[:80]:
            print("  " + ln.rstrip("\n"))
