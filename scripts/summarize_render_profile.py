#!/usr/bin/env python3
"""Summarize MOLAR_VIS_PROFILE JSONL records; accepts log files or stdin.

Example: python scripts/summarize_render_profile.py /tmp/render-profile.log
CPU scopes may be nested; never sum their times to estimate total frame time.
GPU stage samples describe submissions, not necessarily complete frames.
"""
import argparse
import collections
import json
import math
import statistics
import sys


def summarize(lines, warmup=5):
    samples = collections.defaultdict(list)
    counters = collections.Counter()
    metadata = []
    for line in lines:
        marker = "MOLAR_VIS_PROFILE "
        if marker not in line:
            continue
        record = json.loads(line.split(marker, 1)[1])
        if record["kind"] in ("cpu", "gpu"):
            ms = float(record["ms"])
            if math.isfinite(ms) and ms >= 0:
                samples[(record["kind"], record["stage"])].append(ms)
        else:
            values = record["values"]
            for key in ("upload_bytes", "buffer_allocations"):
                counters[key] += values.get(key, 0)
            if record["stage"] == "adapter":
                metadata.append(values)
    results = []
    for (kind, stage), values in sorted(samples.items()):
        values = sorted(values[warmup:])
        if not values:
            continue
        results.append({"kind": kind, "stage": stage, "n": len(values),
                        "median_ms": statistics.median(values),
                        "p95_ms": values[math.ceil(len(values) * .95) - 1],
                        "min_ms": values[0], "max_ms": values[-1]})
    return {"adapter": metadata, "warmup_per_stage": warmup,
            "timings": results, "counters": dict(counters)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("logs", nargs="*")
    parser.add_argument("--warmup", type=int, default=5)
    args = parser.parse_args()
    if args.warmup < 0:
        parser.error("--warmup must be nonnegative")
    if args.logs:
        def lines():
            for path in args.logs:
                with open(path) as stream:
                    yield from stream
        source = lines()
    else:
        source = sys.stdin
    print(json.dumps(summarize(source, args.warmup), indent=2))


if __name__ == "__main__":
    main()
