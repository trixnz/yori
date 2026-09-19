# Application assets

`app-icon.png` is the canonical application icon master. It is a 1024×1024 RGBA
PNG with transparent padding around the icon.

Use this file directly for documentation. `scripts/generate-app-icons.py` derives
the Linux hicolor PNG set and Windows `.ico` file in `assets/platform/` from this
master. Run it from `nix develop` after changing the master, and do not edit the
derived files directly or resize one derived icon to produce another.
