#!/usr/bin/env python3
"""Generate Linux and Windows application icons from the canonical PNG master."""

from pathlib import Path

from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets" / "app-icon.png"
PLATFORM_ASSETS = ROOT / "assets" / "platform"
APP_ID = "io.github.trixnz.yori"
LINUX_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
WINDOWS_SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)


def resized(image: Image.Image, size: int) -> Image.Image:
    return image.resize((size, size), Image.Resampling.LANCZOS)


def generate_linux_icons(master: Image.Image) -> None:
    root = PLATFORM_ASSETS / "linux" / "hicolor"

    for size in LINUX_SIZES:
        destination = root / f"{size}x{size}" / "apps" / f"{APP_ID}.png"
        destination.parent.mkdir(parents=True, exist_ok=True)
        resized(master, size).save(destination, format="PNG", optimize=True)


def generate_windows_icon(master: Image.Image) -> None:
    destination = PLATFORM_ASSETS / "windows" / "yori.ico"
    destination.parent.mkdir(parents=True, exist_ok=True)
    master.save(
        destination,
        format="ICO",
        sizes=[(size, size) for size in WINDOWS_SIZES],
        bitmap_format="png",
    )


def main() -> None:
    with Image.open(SOURCE) as source:
        master = source.convert("RGBA")

    if master.size != (1024, 1024):
        raise ValueError(f"{SOURCE} must be 1024x1024, got {master.size}")

    generate_linux_icons(master)
    generate_windows_icon(master)


if __name__ == "__main__":
    main()
