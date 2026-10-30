#!/usr/bin/env python3
"""Sample a local Linux service process during a benchmark; never reads its environment."""
import argparse
import json
import os
from pathlib import Path
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--seconds", type=float, default=240)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.pid < 1 or args.seconds <= 0:
        parser.error("pid and seconds must be positive")
    process = Path("/proc") / str(args.pid)
    clock_ticks = os.sysconf("SC_CLK_TCK")
    samples = []
    started = time.monotonic()
    previous = None
    while time.monotonic() - started < args.seconds:
        try:
            # Fields after the closing ')' start at proc stat field 3.
            stat = (process / "stat").read_text().rsplit(")", 1)[1].split()
            ticks = int(stat[11]) + int(stat[12])
            status = dict(line.split(":", 1) for line in (process / "status").read_text().splitlines() if ":" in line)
        except FileNotFoundError:
            break
        now = time.monotonic()
        sample = {
            "elapsed_seconds": now - started,
            "rss_kib": int(status.get("VmRSS", "0 kB").split()[0]),
            "threads": int(status["Threads"]),
            "cpu_seconds": ticks / clock_ticks,
        }
        if previous:
            sample["cpu_percent_of_one_core"] = 100 * (ticks - previous[1]) / clock_ticks / (now - previous[0])
        previous = now, ticks
        samples.append(sample)
        Path(args.output).write_text(json.dumps({
            "measurement": "Linux service process only; excludes PostgreSQL and client generator",
            "interval_seconds": 1,
            "peak_rss_kib": max(s["rss_kib"] for s in samples),
            "samples": samples,
        }, indent=2) + "\n")
        time.sleep(1)


if __name__ == "__main__":
    main()
