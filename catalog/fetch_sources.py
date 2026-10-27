"""Download the pinned BambuStudio sources needed by build_catalog.py (dev/CI tool only)."""
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FILES = ["src/libslic3r/PrintConfig.cpp", "src/libslic3r/PrintConfig.hpp", "version.inc"]


def main():
    studio = tomllib.loads((ROOT / "driver" / "upstream.toml").read_text(encoding="utf-8"))["studio"]
    base = f"https://raw.githubusercontent.com/{studio['repo']}/{studio['tag_commit']}/"
    out = ROOT / "catalog" / "upstream-src"
    for rel in FILES:
        target = out / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(base + rel, timeout=120) as response:
            target.write_bytes(response.read())
        print(f"fetched {rel} ({target.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
