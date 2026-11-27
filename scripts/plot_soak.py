#!/usr/bin/env python3
"""Plot RSS vs requests from soak CSV → PNG under benchmarks-memory/."""
import csv, sys
from pathlib import Path
try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
except ImportError:
    print("matplotlib not installed; writing SVG fallback", file=sys.stderr)
    plt = None

def main(path):
    rows = []
    with open(path) as f:
        for r in csv.DictReader(f):
            rows.append((float(r["timestamp_s"]), int(r["requests"]), int(r["rss_kib"])))
    if not rows:
        print("empty", file=sys.stderr); return 1
    out = Path(path).with_suffix(".png")
    xs = [r[1] for r in rows]; ys = [r[2] for r in rows]
    if plt:
        fig, ax = plt.subplots(figsize=(8, 3.5))
        ax.plot(xs, ys, lw=1.2)
        ax.axvline(xs[0] if rows[0][0] >= 60 else next((r[1] for r in rows if r[0] >= 60), xs[0]),
                   color="#888", ls="--", lw=0.8, label="warm-up end")
        ax.set_xlabel("requests"); ax.set_ylabel("VmRSS (KiB)")
        ax.set_title(Path(path).name)
        ax.legend(loc="best", fontsize=8)
        fig.tight_layout(); fig.savefig(out, dpi=120)
        print(out)
    else:
        # minimal SVG
        w, h = 800, 300
        miny, maxy = min(ys), max(ys)
        span = max(maxy - miny, 1)
        pts = " ".join(f"{40+740*i/(len(xs)-1 or 1):.1f},{h-30-240*(y-miny)/span:.1f}" for i,y in enumerate(ys))
        svg = f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><polyline fill="none" stroke="#222" stroke-width="1.5" points="{pts}"/></svg>'
        out = Path(path).with_suffix(".svg")
        out.write_text(svg); print(out)
    return 0

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1]))
