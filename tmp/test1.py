#!/usr/bin/env python3
"""Rename mods that have "DISABLED" prefix to remove it."""

from pathlib import Path

if __name__ == "__main__":
    for i in Path(r"D:\Games\MODS\XXMI").rglob("modname"):
        mod_name = i.read_text(encoding="utf-8").strip()
        if mod_name.startswith("DISABLED"):
            mod_name = mod_name[9:]
            i.write_text(mod_name, encoding="utf-8")
            print(f"Renamed {i} to {mod_name}")
