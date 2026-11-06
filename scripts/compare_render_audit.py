#!/usr/bin/env python3
"""Compare audit benchmark output and make a before/after/difference image.

Requires Pillow. Performance percentages are observations, not automatic acceptance
gates: repeat the release binaries alternately and account for run-to-run noise.
"""
import argparse
import json
from pathlib import Path

from PIL import Image, ImageChops, ImageEnhance, ImageDraw


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()
    failures = []
    pictures = []
    expected = {p.name for p in args.before.iterdir() if p.suffix in (".png", ".bin")
                and p.name != "comparison.png"}
    actual = {p.name for p in args.after.iterdir() if p.suffix in (".png", ".bin")
              and p.name != "comparison.png"}
    assert expected and expected == actual, (expected - actual, actual - expected)
    for name in sorted(expected):
        before, after = args.before / name, args.after / name
        if before.suffix == ".bin":
            if before.read_bytes() != after.read_bytes():
                failures.append(name)
            continue
        a, b = Image.open(before).convert("RGBA"), Image.open(after).convert("RGBA")
        if a.size != b.size or a.tobytes() != b.tobytes():
            failures.append(name)
        assert a.size == b.size, name
        difference = ImageChops.difference(a, b).convert("RGB")
        row = Image.new("RGB", (a.width * 3, a.height + 24), "white")
        ImageDraw.Draw(row).text((5, 5), f"{name}: before | after | difference x8", fill="black")
        for column, picture in enumerate((a, b, ImageEnhance.Brightness(difference).enhance(8))):
            row.paste(picture, (column * a.width, 24))
        row.thumbnail((960, 320))
        pictures.append(row)
    sheet = Image.new("RGB", (max(p.width for p in pictures), sum(p.height for p in pictures)), "white")
    y = 0
    for picture in pictures:
        sheet.paste(picture, (0, y))
        y += picture.height
    sheet.save(args.after / "comparison.png")
    before = {r["stage"]: r for r in json.loads((args.before / "metrics.json").read_text())}
    after = {r["stage"]: r for r in json.loads((args.after / "metrics.json").read_text())}
    assert before.keys() == after.keys()
    for stage, a in before.items():
        b = after[stage]
        print(f'{stage}: median {a["median_ms"]:.3f} -> {b["median_ms"]:.3f} ms '
              f'({100 * (b["median_ms"] / a["median_ms"] - 1):+.1f}%); '
              f'p95 {a["p95_ms"]:.3f} -> {b["p95_ms"]:.3f} ms')
    print(f"Exact parity: {len(expected) - len(failures)}/{len(expected)} artifacts.")
    print(f"Visual inspection: {args.after / 'comparison.png'}")
    if failures:
        raise SystemExit(f"Parity failed: {failures}")


if __name__ == "__main__":
    main()
