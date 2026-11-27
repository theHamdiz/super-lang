"""Super language site + explainable benchmark report (taste-skill: product-performs)."""
from __future__ import annotations

import html
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Optional

ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
# Canonical marketing / benchmark site: repo-top docs/ (GitHub Pages + Jekyll)
WWW = REPO / "docs"
ASSETS_SRC = ROOT / "assets"
SITE_ASSETS = ROOT / "site_assets"
# Legacy path kept as a symlink to docs/ for older scripts and muscle memory
WWW_LEGACY = ROOT / "www"
# Public GitHub Pages mirror: sibling Mandara repo (Personal/mandara → MandaraShell worktree)
MANDARA_ROOT = Path(
    __import__("os").environ.get(
        "SUPER_MANDARA_ROOT",
        str(REPO.parent / "mandara"),
    )
)
MANDARA_SUPER = MANDARA_ROOT / "super"


WEIGHT_LABELS = (
    ("speed", "Speed", "40%"),
    ("memory", "Memory", "25%"),
    ("reliability", "Reliability", "15%"),
    ("cycle", "Cycle", "10%"),
    ("loc", "LoC", "10%"),
)


def scrub_results_for_docs(obj: Any, *, repo: Path | None = None) -> Any:
    """Strip absolute machine paths from results.json before publishing under docs/.

    Repo roots → ``<repo>/…``; other ``/home/…`` toolchain paths → ``<home-path>``.
    """
    roots: list[str] = []
    base = (repo or REPO).resolve()
    # Common twin mounts (Personal under /mnt/storage vs $HOME/projects).
    for candidate in (
        base,
        Path("/mnt/storage/projects/Personal/super"),
        Path.home() / "projects/Personal/super",
    ):
        try:
            s = str(candidate.resolve())
        except OSError:
            s = str(candidate)
        if s not in roots:
            roots.append(s)

    def scrub(value: Any) -> Any:
        if isinstance(value, dict):
            return {k: scrub(v) for k, v in value.items()}
        if isinstance(value, list):
            return [scrub(v) for v in value]
        if isinstance(value, str):
            s = value
            for root in sorted(roots, key=len, reverse=True):
                if s == root or s.startswith(root + "/"):
                    s = "<repo>" + s[len(root) :]
                    break
                if root in s:
                    s = s.replace(root, "<repo>")
            if s.startswith("/home/"):
                s = "<home-path>"
            return s
        return value

    return scrub(obj)

# UI labels: camelCase, no underscores
DISPLAY_NAMES: dict[str, str] = {
    "super_aot": "SuperAot",
    "super_jit": "SuperJit",
    "c": "C",
    "rust": "Rust",
    "zig": "Zig",
    "bun": "Bun",
    "kotlin": "Kotlin",
    "lua": "Lua",
    "python": "Python",
    "julia": "Julia",
    "super": "Super",
}


def display_name(raw: Any) -> str:
    key = "" if raw is None else str(raw)
    if key == "all":
        return "All"
    if key in DISPLAY_NAMES:
        return DISPLAY_NAMES[key]
    if "_" in key:
        return "".join(p[:1].upper() + p[1:] for p in key.split("_") if p)
    if not key:
        return "n/a"
    return key[:1].upper() + key[1:]


def public_category_winners(cat_w: dict[str, Any]) -> dict[str, Any]:
    """Visitor-facing category map (Reliability tie at 100% → All)."""
    out = dict(cat_w)
    if out.get("reliability"):
        out["reliability"] = "all"
    return out


def category_tile_id_and_label(axis: str, winner: str) -> tuple[str, str]:
    if axis == "reliability" and winner:
        return "all", "All"
    return winner, display_name(winner)


def build_cat_tiles_html(cat_w: dict[str, Any], *, chart_href: str | None = None) -> str:
    """Category winner tiles. With `chart_href` they are links into the chart (no JS needed)."""
    cat_order = ("speed", "memory", "reliability", "cycle", "loc")
    cat_labels = {k: lab for k, lab, _ in WEIGHT_LABELS}
    cat_tiles: list[str] = []
    for k in cat_order:
        v = cat_w.get(k)
        if not v:
            continue
        mark = cat_labels.get(k, k)
        lang_id, dn = category_tile_id_and_label(k, v)
        if chart_href:
            open_tag = (
                f'<a class="cat-tile" href="{_esc(chart_href)}?axis={_esc(k)}#live-chart" '
                f'data-axis="{_esc(k)}" data-lang="{_esc(lang_id)}" data-reveal '
                f'aria-label="{_esc(mark)} winner: {_esc(dn)} (open in chart)">'
            )
            close_tag = "</a>"
        else:
            open_tag = (
                f'<button type="button" class="cat-tile" data-axis="{_esc(k)}" data-lang="{_esc(lang_id)}" '
                f'data-reveal aria-label="{_esc(mark)} winner: {_esc(dn)}">'
            )
            close_tag = "</button>"
        cat_tiles.append(
            f'{open_tag}<span class="cat-mark" aria-hidden="true">{_esc(mark)}</span>'
            f"<strong>{_esc(dn)}</strong>{close_tag}"
        )
    if not cat_tiles:
        cat_tiles.append(
            '<button type="button" class="cat-tile" data-axis="score" disabled>'
            '<span class="cat-mark" aria-hidden="true">—</span>'
            "<strong>awaiting run</strong></button>"
        )
    return "".join(cat_tiles)


def _esc(s: Any) -> str:
    """Attribute-safe escape (quotes → entities). Use for href/attr interpolation."""
    return html.escape("" if s is None else str(s), quote=True)


def _esc_text(s: Any) -> str:
    """Body/code escape: &, <, > only. Leaves " and ' as real characters (no &quot;)."""
    return html.escape("" if s is None else str(s), quote=False)


def _fmt_s(v: Optional[float], digits: int = 4) -> str:
    if v is None:
        return "n/a"
    return f"{v:.{digits}f}s"


def site_css() -> str:
    return r"""
/* Self-hosted Cascadia (OFL) + JetBrains Mono fallback — works offline on Pages */
@font-face {
  font-family: "Cascadia Mono";
  src: url("assets/fonts/CascadiaMono-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}
@font-face {
  font-family: "Cascadia Mono";
  src: url("assets/fonts/CascadiaMono-SemiBold.woff2") format("woff2");
  font-weight: 600;
  font-style: normal;
  font-display: swap;
}
@font-face {
  font-family: "Cascadia Code";
  src: url("assets/fonts/CascadiaCode-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}
@font-face {
  font-family: "Cascadia Code";
  src: url("assets/fonts/CascadiaCode-SemiBold.woff2") format("woff2");
  font-weight: 600;
  font-style: normal;
  font-display: swap;
}
@font-face {
  font-family: "JetBrains Mono";
  src: url("assets/fonts/JetBrainsMono-Regular.ttf") format("truetype");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}
@font-face {
  font-family: "JetBrains Mono";
  src: url("assets/fonts/JetBrainsMono-Medium.ttf") format("truetype");
  font-weight: 500;
  font-style: normal;
  font-display: swap;
}
:root {
  --bg: #ff7a1f;
  --surface: #ff8f3d;
  --ink: #050f16;
  --muted: #5a2306;
  --accent: #050f16;
  --accent-ink: #ff7a1f;
  --field: #ffffff;
  --field-ink: #050f16;
  --gap: 8px;
  --radius: 0;
  --step-small: 1rem;
  --step-body: 1.125rem;
  --step-lead: clamp(1.25rem, 1.1rem + 0.55vw, 1.5rem);
  --step-head: clamp(2rem, 1.4rem + 2.6vw, 3.5rem);
  --display: clamp(2.75rem, 1.5rem + 4.5vw, 5.25rem);
  --margin: clamp(0.5rem, 1.2vw, 0.75rem);
  --ease: cubic-bezier(0.22, 1, 0.36, 1);
  --ease-out: cubic-bezier(0.22, 1, 0.36, 1);
  --font: "Bricolage Grotesque", sans-serif;
  --font-mono: "Cascadia Mono", "Cascadia Code", "JetBrains Mono", ui-monospace, monospace;
}
*, *::before, *::after { box-sizing: border-box }
html { scroll-behavior: smooth }
body {
  margin: 0;
  font-family: var(--font);
  font-weight: 400;
  font-size: var(--step-body);
  line-height: 1.5;
  letter-spacing: -0.005em;
  color: var(--ink);
  background: var(--bg);
}
section > p, .section p, .hero-support, .total-cap, .faq-answer, .tile p {
  font-size: var(--step-body);
  color: var(--ink);
}
a { color: inherit; text-decoration-thickness: 1px; text-underline-offset: 0.18em }
a:hover, a:focus-visible { color: var(--accent) }
button {
  font: inherit;
  color: inherit;
  background: none;
  border: 0;
  cursor: pointer;
  padding: 0;
  text-align: left;
}
img, video, canvas, svg { display: block; max-width: 100%; height: auto }

/* —— Nav —— */
.site-nav {
  display: grid;
  grid-template-columns: auto 1fr auto;
  gap: var(--gap);
  padding: var(--margin);
  font-size: var(--step-body);
}
.site-nav .mark {
  display: flex;
  align-items: center;
  background: var(--ink);
  color: var(--accent-ink);
  padding: 0.7rem 1rem;
  font-weight: 500;
  text-decoration: none;
  letter-spacing: -0.03em;
}
.site-nav nav {
  display: flex;
  flex-wrap: wrap;
  gap: 1rem 1.25rem;
  align-items: center;
  background: var(--surface);
  padding: 0.7rem 1rem;
}
.site-nav nav a { text-decoration: none }
.site-nav nav a[aria-current="page"] { font-weight: 600; text-decoration: underline }
.site-nav .cta {
  display: inline-flex;
  align-items: center;
  background: var(--ink);
  color: var(--accent-ink);
  padding: 0.7rem 1.05rem;
  text-decoration: none;
  font-weight: 500;
}
.site-nav .cta:hover, .site-nav .cta:focus-visible {
  outline: 3px solid var(--ink);
  outline-offset: 3px;
  color: var(--accent-ink);
}
.cta {
  display: inline-block;
  background: var(--accent);
  color: var(--accent-ink);
  padding: 0.85rem 1.25rem;
  text-decoration: none;
  font-weight: 500;
}
.cta:hover, .cta:focus-visible {
  outline: 3px solid var(--accent);
  outline-offset: 3px;
  color: var(--accent-ink);
}
.cta:active, .lang-line:active, .pick:active, .cat-tile:active { transform: translateY(2px) }
.cta-ghost {
  display: inline-block;
  padding: 0.85rem 1.25rem;
  text-decoration: none;
  font-weight: 500;
  color: var(--ink);
  background: var(--surface);
}

/* —— Hero board: overall winner + category winners —— */
.hero-board {
  display: grid;
  grid-template-columns: 1.15fr 0.85fr;
  grid-template-rows: auto auto;
  gap: var(--gap);
  padding: 0 var(--margin) var(--margin);
  min-height: calc(100svh - 5rem);
}
.tile {
  padding: 1.25rem 1.35rem;
  position: relative;
  overflow: hidden;
}
.tile-winner {
  grid-row: 1 / 3;
  background: var(--field);
  color: var(--field-ink);
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  min-height: 70svh;
  animation: shutter 1.1s var(--ease-out) both;
  isolation: isolate;
}
/* Mandara-style barely-visible geometric watermark (flower-of-life) */
.tile-winner::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  background-color: var(--field-ink);
  opacity: 0.055;
  -webkit-mask-image: url("assets/ui/geo-mandala.svg");
  mask-image: url("assets/ui/geo-mandala.svg");
  -webkit-mask-repeat: no-repeat;
  mask-repeat: no-repeat;
  -webkit-mask-position: right -12% bottom -18%;
  mask-position: right -12% bottom -18%;
  -webkit-mask-size: min(92%, 38rem);
  mask-size: min(92%, 38rem);
}
.tile-winner > * { position: relative; z-index: 1 }
@keyframes shutter {
  from { clip-path: inset(0 100% 0 0); transform: scale(1.06); }
  to { clip-path: inset(0 0 0 0); transform: scale(1); }
}
.tile-winner .win-label {
  font-size: var(--step-body);
  margin: 0 0 0.5rem;
  color: var(--muted);
}
.tile-winner h1 {
  margin: 0;
  font-size: clamp(4rem, 14vw, 11rem);
  font-weight: 600;
  letter-spacing: -0.05em;
  line-height: 0.85;
  max-width: 8ch;
}
.tile-winner .win-score {
  margin: 1rem 0 0;
  font-size: clamp(2.5rem, 8vw, 6rem);
  font-weight: 500;
  letter-spacing: -0.04em;
  line-height: 0.9;
  font-variant-numeric: tabular-nums;
}
.tile-winner .hero-support {
  margin: 1.25rem 0 0;
  max-width: 28ch;
  color: var(--field-ink);
}
.tile-winner .hero-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.65rem;
  margin-top: 1.75rem;
}
.tile-cats {
  background: var(--ink);
  color: var(--accent-ink);
  display: flex;
  flex-direction: column;
  gap: 0.55rem;
  min-height: 42svh;
}
.tile-cats h2 {
  margin: 0 0 0.5rem;
  font-size: var(--step-lead);
  font-weight: 500;
  letter-spacing: -0.03em;
  line-height: 1;
  max-width: 14ch;
  color: var(--accent-ink);
}
.cat-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--gap);
  flex: 1;
}
.cat-tile {
  position: relative;
  overflow: hidden;
  isolation: isolate;
  background: #1a0a04;
  color: var(--accent-ink);
  padding: 1rem 1.05rem 0.9rem;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  min-height: 6.75rem;
  transition: background 160ms ease, color 160ms ease, transform 120ms var(--ease);
}
/* Girih star watermark — corner-biased, barely visible on dark tiles */
a.cat-tile { text-decoration: none }
.cat-tile::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  background-color: var(--accent-ink);
  opacity: 0.1;
  -webkit-mask-image: url("assets/ui/geo-star.svg");
  mask-image: url("assets/ui/geo-star.svg");
  -webkit-mask-repeat: no-repeat;
  mask-repeat: no-repeat;
  -webkit-mask-position: right -28% bottom -35%;
  mask-position: right -28% bottom -35%;
  -webkit-mask-size: 7.5rem;
  mask-size: 7.5rem;
  transition: background-color 160ms ease, opacity 160ms ease;
}
.cat-tile:nth-child(1)::before {
  -webkit-mask-position: right -30% top -40%;
  mask-position: right -30% top -40%;
}
.cat-tile:nth-child(2)::before {
  -webkit-mask-position: left -35% bottom -30%;
  mask-position: left -35% bottom -30%;
}
.cat-tile:nth-child(3)::before {
  -webkit-mask-position: right -20% bottom -45%;
  mask-position: right -20% bottom -45%;
}
.cat-tile:nth-child(4)::before {
  -webkit-mask-position: left -25% top -35%;
  mask-position: left -25% top -35%;
}
.cat-tile:nth-child(5)::before {
  -webkit-mask-position: right -8% center;
  mask-position: right -8% center;
  -webkit-mask-size: 9.5rem;
  mask-size: 9.5rem;
}
.cat-tile:nth-child(5) { grid-column: 1 / -1 }
/* Editorial category watermark — oversized type as atmosphere behind the winner */
.cat-tile .cat-mark {
  position: absolute;
  left: 0.55rem;
  right: -0.35rem;
  top: 48%;
  transform: translateY(-52%);
  z-index: 1;
  margin: 0;
  font-size: clamp(2.6rem, 5.2vw, 4.15rem);
  font-weight: 600;
  letter-spacing: -0.055em;
  line-height: 0.78;
  opacity: 0.68;
  color: inherit;
  pointer-events: none;
  white-space: nowrap;
  overflow: hidden;
  transition: opacity 160ms ease;
}
.cat-tile strong {
  position: relative;
  z-index: 2;
  font-size: clamp(1.35rem, 2.4vw, 1.85rem);
  font-weight: 600;
  letter-spacing: -0.03em;
  line-height: 1;
}
.cat-tile:hover, .cat-tile:focus-visible {
  background: var(--field);
  color: var(--field-ink);
  outline: 3px solid var(--accent-ink);
  outline-offset: 2px;
}
.cat-tile:hover::before, .cat-tile:focus-visible::before {
  background-color: var(--field-ink);
  opacity: 0.06;
}
.cat-tile:hover .cat-mark,
.cat-tile:focus-visible .cat-mark {
  opacity: 0.55;
}
.cat-tile.is-active {
  background: var(--accent-ink);
  color: var(--ink);
}
.cat-tile.is-active::before {
  background-color: var(--ink);
  opacity: 0.08;
}
.cat-tile.is-active .cat-mark {
  opacity: 0.6;
}
.tile-axis {
  background: var(--surface);
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  min-height: 22svh;
}
.tile-axis .axis-live {
  font-size: clamp(1.5rem, 3vw, 2.25rem);
  font-weight: 500;
  letter-spacing: -0.03em;
  line-height: 1.05;
  margin: 0;
  max-width: 16ch;
}
.axis-meter {
  margin-top: 1rem;
  height: 0.85rem;
  background: rgba(5,15,22,0.14);
  overflow: hidden;
}
.axis-meter > i {
  display: block;
  height: 100%;
  width: 0;
  background: var(--ink);
  transition: width 500ms var(--ease);
}

/* —— Sections —— */
.section { padding: 7rem var(--margin); position: relative; z-index: 1; background: var(--bg) }
.section.band-field { background: var(--field); color: var(--field-ink) }
.section.band-surface { background: var(--surface) }
.section.band-ink { background: var(--ink); color: var(--accent-ink) }
.section h2 {
  margin: 0 0 1.5rem;
  font-size: var(--step-head);
  font-weight: 500;
  letter-spacing: -0.03em;
  line-height: 0.95;
  max-width: 14ch;
  text-wrap: balance;
}
.band-field .muted, .band-field .note { color: #5a5248 }
.stats-wrap {
  display: grid;
  grid-template-columns: 1.05fr 0.95fr;
  gap: 3rem;
  align-items: start;
}
.total {
  font-size: clamp(4rem, 12vw, 10rem);
  font-weight: 500;
  letter-spacing: -0.05em;
  line-height: 0.85;
  margin: 0;
  font-variant-numeric: tabular-nums;
}
.total-cap { margin: 0.75rem 0 0; color: var(--ink); max-width: 32ch }
.live-hint { margin: 0.5rem 0 0; color: var(--muted); font-size: var(--step-small) }
.picks { display: flex; flex-wrap: wrap; gap: 0.5rem; margin-top: 1.25rem }
.pick {
  background: var(--ink);
  color: var(--accent-ink);
  padding: 0.5rem 0.75rem;
  font-size: var(--step-small);
  font-weight: 500;
  transition: background 150ms ease, color 150ms ease;
}
.pick.is-active, .pick:hover, .pick:focus-visible {
  background: var(--field);
  color: var(--field-ink);
  outline: 3px solid var(--ink);
  outline-offset: 2px;
}
.receipt {
  background: var(--field);
  color: var(--field-ink);
  padding: 1.25rem 1.35rem 0;
  clip-path: polygon(0 0, 100% 0, 100% calc(100% - 14px), 96% 100%, 92% calc(100% - 14px), 88% 100%, 84% calc(100% - 14px), 80% 100%, 76% calc(100% - 14px), 72% 100%, 68% calc(100% - 14px), 64% 100%, 60% calc(100% - 14px), 56% 100%, 52% calc(100% - 14px), 48% 100%, 44% calc(100% - 14px), 40% 100%, 36% calc(100% - 14px), 32% 100%, 28% calc(100% - 14px), 24% 100%, 20% calc(100% - 14px), 16% 100%, 12% calc(100% - 14px), 8% 100%, 4% calc(100% - 14px), 0 100%);
}
.receipt h3 { margin: 0 0 0.75rem; font-size: 1rem; font-weight: 500 }
.receipt-row {
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 0.75rem;
  padding: 0.55rem 0;
  font-variant-numeric: tabular-nums;
  font-size: 0.9375rem;
  border-bottom: 2px dotted currentColor;
}
.receipt-row span:last-child { font-weight: 500 }
.receipt-total {
  margin: 0.85rem -1.35rem 0;
  padding: 0.85rem 1.35rem 1.6rem;
  background: var(--accent);
  color: var(--accent-ink);
  display: flex;
  justify-content: space-between;
  font-weight: 500;
}

/* Interactive composite SVG chart */
.chart-panel {
  background: #1a0a04;
  padding: 1.25rem;
  min-height: 28rem;
}
.chart-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  margin-bottom: 1rem;
}
.chart-toolbar button {
  background: transparent;
  color: var(--accent-ink);
  padding: 0.4rem 0.7rem;
  font-weight: 500;
  opacity: 0.65;
  transition: opacity 150ms ease, background 150ms ease;
}
.chart-toolbar button.is-active,
.chart-toolbar button:hover,
.chart-toolbar button:focus-visible {
  opacity: 1;
  background: var(--accent-ink);
  color: var(--ink);
  outline: none;
}
.rank-svg { width: 100%; height: auto; overflow: visible }
.rank-svg .bar-bg { fill: #2a2620 }
.rank-svg .bar-fg {
  fill: var(--accent-ink);
  transition: width 450ms var(--ease), fill 200ms ease;
  transform-origin: left center;
}
.rank-svg .bar-fg.is-winner { fill: var(--field) }
.rank-svg .bar-fg.is-dim { fill: #5a3a22 }
.rank-svg text {
  fill: var(--accent-ink);
  font-family: var(--font);
  font-size: 14px;
}
.rank-svg .bar-hit { cursor: pointer; fill: transparent }
.radar-wrap { display: grid; place-items: center; min-height: 16rem }
.radar-svg { width: min(100%, 22rem); height: auto }
.radar-svg .axis { stroke: rgba(5,15,22,0.18); stroke-width: 1; fill: none }
.radar-svg .poly {
  fill: rgba(255,122,31,0.35);
  stroke: var(--ink);
  stroke-width: 2;
  transition: d 400ms var(--ease), points 400ms var(--ease);
}
.radar-svg text { font-family: var(--font); font-size: 12px; fill: var(--field-ink) }

.weight-bar {
  display: flex;
  gap: 6px;
  min-height: 28vh;
  margin-top: 2rem;
}
.wseg {
  background: var(--surface);
  color: var(--ink);
  padding: 1.25rem;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  min-width: 12%;
  transition: flex-grow 400ms var(--ease), transform 160ms var(--ease);
}
.wseg:nth-child(2) { background: #1a0a04; color: #ffb07a }
.wseg:nth-child(3) { background: var(--field); color: var(--field-ink) }
.wseg:nth-child(4) { background: #2a2620; color: #ffb07a }
.wseg:nth-child(5) { background: #2a2620; color: var(--ink) }
.wseg strong { font-size: clamp(1.25rem, 2vw, 1.875rem); font-weight: 600 }
.wseg span { font-size: 1.0625rem; max-width: 14ch }
.wseg:hover, .wseg:focus-within { transform: translateY(-3px) }

.ui-stage {
  background: var(--ink);
  color: var(--accent-ink);
  padding: 1.5rem;
  min-height: 22rem;
  position: relative;
  overflow: hidden;
}
.ui-stage canvas { width: 100%; height: 18rem }
.ui-caption {
  margin: 0.75rem 0 0;
  font-size: var(--step-body);
  color: #ffb07a;
  max-width: 42ch;
}

.lang-list {
  display: flex;
  flex-direction: column;
  padding: 5rem var(--margin);
  gap: 0;
}
.lang-line {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  font-size: clamp(2.25rem, 7vw, 6rem);
  font-weight: 500;
  letter-spacing: -0.04em;
  line-height: 0.92;
  padding: 0.08em 0;
  width: 100%;
  transition: color 160ms ease;
}
.lang-line:hover .lang-name,
.lang-line:focus-visible .lang-name,
.lang-line.is-active .lang-name { color: var(--field) }
.band-ink { background: var(--ink); color: var(--accent-ink) }
.lang-score {
  font-size: clamp(1rem, 1.5vw, 1.25rem);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  color: var(--muted);
}
.faq-grid {
  display: grid;
  grid-template-columns: minmax(0, 0.38fr) minmax(0, 0.62fr);
  gap: 2.5rem;
  align-items: start;
}
.faq-q {
  display: block;
  width: 100%;
  padding: 0.55rem 0 0.55rem 1.4rem;
  position: relative;
  color: var(--muted);
  font-size: 1.125rem;
}
.faq-q::before {
  content: "";
  position: absolute;
  left: 0;
  top: 0.95rem;
  width: 8px;
  height: 8px;
  background: var(--muted);
}
.faq-q.is-active { color: var(--ink) }
.faq-q.is-active::before { background: var(--accent) }
.faq-answer {
  font-size: var(--step-lead);
  line-height: 1.35;
  max-width: 30ch;
  margin: 0;
}
.faq-bands .faq-band {
  padding: 3.5rem var(--margin);
  display: grid;
  grid-template-columns: minmax(0, 0.4fr) minmax(0, 0.6fr);
  gap: 2rem;
}
.faq-bands .faq-band:nth-child(odd) { background: var(--bg) }
.faq-bands .faq-band:nth-child(even) { background: var(--surface) }
.faq-bands .faq-band h3 {
  margin: 0;
  font-size: clamp(1.35rem, 2vw, 1.75rem);
  font-weight: 600;
  letter-spacing: -0.03em;
  line-height: 1.1;
  max-width: 18ch;
}
.faq-bands .faq-band p {
  margin: 0;
  max-width: 42ch;
  font-size: var(--step-body);
  line-height: 1.5;
}
.timings-layout { display: grid; grid-template-columns: 1fr; gap: 3rem }
.algo { margin: 0 0 3rem; padding: 0 }
.algo h3 {
  margin: 0 0 0.35rem;
  font-size: clamp(1.5rem, 1.1rem + 1.4vw, 2.25rem);
  font-weight: 500;
  letter-spacing: -0.03em;
  line-height: 0.95;
}
.note { color: var(--muted); margin: 0 0 1rem; font-size: var(--step-small) }
.achart { display: flex; flex-direction: column; gap: 0.45rem; margin: 1rem 0 1.5rem }
.abar {
  display: grid;
  grid-template-columns: 7.5rem 1fr 5.5rem;
  gap: 0.65rem;
  align-items: center;
  font-size: var(--step-small);
  font-variant-numeric: tabular-nums;
}
.atrack { background: #2a2620; height: 0.7rem; overflow: hidden }
.afill {
  background: var(--accent);
  height: 100%;
  transform-origin: left center;
  animation: growBar 700ms var(--ease) both;
}
@keyframes growBar { from { transform: scaleX(0) } to { transform: scaleX(1) } }
.band-field .atrack { background: rgba(12,10,8,0.12) }
.band-field .afill { background: var(--field-ink) }

/* Tables + cardized matrix */
.table-wrap { margin-top: 1rem; max-width: 100% }
table.data {
  width: 100%;
  border-collapse: collapse;
  font-size: var(--step-small);
  font-variant-numeric: tabular-nums;
}
table.data th, table.data td {
  word-break: break-word;
  text-align: left;
  padding: 0.65rem 0.75rem;
  border-bottom: 2px solid rgba(255,255,255,0.12);
  vertical-align: top;
}
.band-field table.data th,
.band-field table.data td {
  border-bottom-color: rgba(5,15,22,0.16);
  color: var(--field-ink);
}
table.data th { font-weight: 500; color: var(--ink) }
table.data td.num, table.data th.num { text-align: right }
table.data td.is-best-cell { font-weight: 600 }
.band-field table.data th { color: var(--field-ink) }
.band-field table.data td.is-best-cell { color: #8a3a00 }

.timing-cards { display: none; gap: 0.75rem; margin-top: 1.25rem }
.timing-card {
  background: rgba(5,15,22,0.06);
  padding: 1rem 1.1rem;
}
.timing-card h4 {
  margin: 0 0 0.75rem;
  font-size: 1.125rem;
  font-weight: 600;
  letter-spacing: -0.02em;
}
.timing-card dl {
  margin: 0;
  display: grid;
  grid-template-columns: 1fr auto;
  gap: 0.35rem 1rem;
  font-variant-numeric: tabular-nums;
  font-size: var(--step-body);
}
.timing-card dt { color: var(--muted) }
.timing-card dd { margin: 0; font-weight: 500; text-align: right }
.timing-card dd.is-best { color: #8a3a00; font-weight: 600 }
.matrix-filter {
  display: flex;
  flex-wrap: wrap;
  gap: 0.45rem;
  margin: 1rem 0 0.5rem;
}
.matrix-filter button {
  background: rgba(5,15,22,0.08);
  padding: 0.4rem 0.7rem;
  font-weight: 500;
}
.matrix-filter button.is-active {
  background: var(--ink);
  color: var(--accent-ink);
}

.showcase {
  min-height: 92svh;
  background: var(--ink);
  color: var(--accent-ink);
  padding: 2.5rem var(--margin);
  overflow: hidden;
  position: relative;
}
.showcase h2 { color: var(--accent-ink); max-width: 12ch }
.showcase p { max-width: 36ch; margin: 0 0 2rem; color: #ffb07a }
.showcase .blown {
  font-size: clamp(4rem, 14vw, 10rem);
  line-height: 0.85;
  letter-spacing: -0.05em;
  font-weight: 500;
  margin: 0;
}
.cta-band {
  min-height: 70svh;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--gap);
  padding: var(--margin);
  background: var(--bg);
  align-items: stretch;
}
.cta-band .cta-main {
  background: var(--ink);
  color: var(--accent-ink);
  padding: 2.5rem;
  display: flex;
  flex-direction: column;
  justify-content: center;
}
.cta-band h2 {
  font-size: var(--display);
  max-width: 10ch;
  margin: 0 0 1rem;
  color: var(--accent-ink);
}
.cta-band .cta-side {
  background: var(--field);
  color: var(--field-ink);
  padding: 2rem;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
}
.book-panel h3 { margin: 0 0 0.5rem; font-weight: 500 }
.book-panel p { margin: 0 0 1rem; color: #5a5248 }

/* Footer — designed close */
.site-footer {
  background: var(--field);
  color: var(--field-ink);
  padding: 3rem var(--margin) 1.5rem;
  min-height: 70svh;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
  gap: 2.5rem;
}
.footer-links {
  display: flex;
  flex-wrap: wrap;
  gap: 1.25rem 2rem;
  font-size: 1.0625rem;
}
.footer-links a { text-decoration: none; color: #5a5248 }
.footer-links a:hover, .footer-links a:focus-visible { color: var(--field-ink) }
.footer-mark {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: 4px;
  width: 100%;
  height: 34svh;
  align-items: end;
}
.footer-mark span {
  display: flex;
  align-items: flex-end;
  justify-content: center;
  background: var(--ink);
  color: var(--accent-ink);
  font-weight: 600;
  letter-spacing: -0.06em;
  line-height: 0.8;
  font-size: clamp(3.5rem, 14vw, 12rem);
  height: 100%;
  padding-bottom: 0.08em;
}
.footer-mark span:nth-child(1) { height: 92% }
.footer-mark span:nth-child(2) { height: 72%; background: var(--bg); color: var(--ink) }
.footer-mark span:nth-child(3) { height: 100% }
.footer-mark span:nth-child(4) { height: 64%; background: var(--surface); color: var(--ink) }
.footer-mark span:nth-child(5) { height: 86% }
.footer-legal {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  flex-wrap: wrap;
  font-size: var(--step-small);
  color: #5a5248;
}

.page-hero {
  padding: 10vh var(--margin) 4rem;
  min-height: 70svh;
  display: flex;
  flex-direction: column;
  justify-content: flex-end;
}
.page-hero h1 {
  margin: 0;
  font-size: var(--display);
  font-weight: 500;
  letter-spacing: -0.05em;
  line-height: 0.9;
  max-width: 12ch;
  text-wrap: balance;
}
.page-hero p {
  margin: 1.25rem 0 0;
  max-width: 38ch;
  font-size: var(--step-body);
  color: var(--ink);
}
.landing-excerpt {
  padding: clamp(3.5rem, 11vh, 7rem) var(--margin) clamp(2.5rem, 6vh, 4.5rem);
  max-width: 46rem;
}
.landing-excerpt h1 {
  margin: 0;
  font-size: var(--display);
  font-weight: 500;
  letter-spacing: -0.05em;
  line-height: 0.92;
  max-width: 11ch;
  text-wrap: balance;
}
.landing-lead {
  margin: 1.35rem 0 0;
  max-width: 42ch;
  font-size: var(--step-lead);
  line-height: 1.45;
  color: var(--ink);
}
.landing-points {
  margin: 1.5rem 0 0;
  padding: 0;
  list-style: none;
  max-width: 44ch;
}
.landing-points li {
  margin: 0.55rem 0;
  font-size: var(--step-body);
  line-height: 1.45;
}
.landing-excerpt .hero-actions {
  margin-top: 1.75rem;
  flex-wrap: wrap;
}
.install-copy {
  margin-top: 2.25rem;
  background: var(--surface);
  padding: 1.1rem 1.15rem 1.25rem;
  max-width: min(52rem, 100%);
}
.install-copy-label {
  margin: 0 0 0.65rem;
  font-size: var(--step-body);
  font-weight: 500;
}
.install-copy pre {
  margin: 0 0 0.85rem;
  overflow-x: auto;
  font-family: var(--font-mono);
  font-size: 0.82rem;
  line-height: 1.45;
  white-space: pre-wrap;
  word-break: break-word;
}
.install-copy button.cta { font-size: var(--step-body) }
.prose { max-width: 62ch }
.prose p { margin: 0 0 1rem }
.code-stage {
  background: var(--surface);
  padding: 1.5rem;
  overflow-x: auto;
}
.code-stage pre {
  margin: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.9rem;
  line-height: 1.45;
  color: var(--ink);
  white-space: pre;
}
.intro-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 1.5rem;
}
.intro-grid strong { display: block; font-size: var(--step-lead); font-weight: 500 }
.intro-grid em { font-style: normal; color: var(--muted) }
.spec-body { padding: 2rem var(--margin) 5rem; max-width: 72ch }
.spec-body h2 {
  margin: 2.5rem 0 0.75rem;
  font-size: var(--step-lead);
  font-weight: 500;
}
.spec-body pre {
  background: var(--surface);
  padding: 1rem;
  overflow-x: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 0.875rem;
}
.example-list { display: grid; gap: 2rem }
.example-list article { background: var(--surface); padding: 1.5rem }
.example-list h3 { margin: 0 0 0.5rem; font-weight: 500; font-size: var(--step-lead) }
.example-list p { margin: 0; color: var(--muted); max-width: 42ch }

/* —— Apple Books–like reader —— */
.books-shelf {
  padding: 4rem var(--margin) 2rem;
  min-height: 88svh;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 2rem;
}
.books-shelf h1 {
  margin: 0;
  font-size: var(--display);
  font-weight: 500;
  letter-spacing: -0.05em;
  line-height: 0.9;
  max-width: 10ch;
}
.shelf-row {
  display: flex;
  gap: 1.5rem;
  align-items: flex-end;
  overflow-x: auto;
  padding-bottom: 1rem;
}
.cover-card {
  width: min(42vw, 220px);
  aspect-ratio: 2 / 3;
  background: var(--ink);
  color: var(--accent-ink);
  padding: 1.25rem;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  flex-shrink: 0;
  cursor: pointer;
  transform-origin: bottom center;
  transition: transform 200ms var(--ease);
}
.cover-card:hover, .cover-card:focus-visible {
  transform: translateY(-10px) rotate(-2deg);
  outline: 3px solid var(--ink);
  outline-offset: 4px;
}
.cover-card strong {
  font-size: clamp(1.35rem, 3vw, 1.85rem);
  letter-spacing: -0.04em;
  line-height: 0.95;
}
.cover-card span { font-size: 0.9375rem; opacity: 0.75 }
.reader {
  display: none;
  position: fixed;
  inset: 0;
  z-index: 40;
  background: #1a120c;
  color: var(--field);
  flex-direction: column;
}
.reader.is-open { display: flex }
.reader-bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 0.75rem;
  padding: 0.45rem 0.85rem;
  background: #0c0806;
  font-size: var(--step-body);
  flex: 0 0 auto;
}
.reader-bar button, .reader-bar a {
  color: var(--accent-ink);
  background: transparent;
  text-decoration: none;
  font-weight: 500;
  padding: 0.35rem 0.55rem;
}
.reader-bar button:hover, .reader-bar button:focus-visible,
.reader-bar a:hover, .reader-bar a:focus-visible {
  outline: 2px solid var(--accent-ink);
  outline-offset: 2px;
}
.reader-stage {
  flex: 1 1 auto;
  position: relative;
  overflow: hidden;
  min-height: 0;
  /* Tight inset — book dominates the viewport */
  padding: 0.35rem 0.5rem 0.15rem;
}
.book-spread {
  /* ~90% of the reader stage (stage already fills viewport minus slim chrome) */
  position: absolute;
  top: 5%;
  left: 5%;
  width: 90%;
  height: 90%;
  transform: none;
}
.sheet {
  position: absolute;
  top: 0;
  width: 50%;
  height: 100%;
  background: transparent;
  color: #1a120c;
}
/* Instant pager — under/back/curl chrome unused */
.sheet.under,
.face.back,
.page-edge,
.page-curl {
  display: none !important;
}
.sheet.under .page-inner {
  padding: clamp(1rem, 2.2vw, 1.85rem) clamp(1rem, 2.2vw, 1.85rem) 2.2rem;
  height: 100%;
  overflow: hidden;
  font-size: clamp(0.78rem, 1.05vw, 0.95rem);
  line-height: 1.45;
  font-family: "Bricolage Grotesque", Georgia, serif;
}
.sheet.under .page-num {
  position: absolute;
  bottom: 0.75rem;
  right: 1rem;
  font-size: 0.75rem;
  opacity: 0.45;
}
.sheet.under.left { left: 0 }
.sheet.under.right { left: 50% }
.sheet.hard {
  z-index: 2;
  cursor: pointer;
  user-select: none;
}
.sheet.left { left: 0 }
.sheet.right { left: 50% }
.sheet.left.under::after, .sheet.hard.left .face.front::after {
  content: "";
  position: absolute;
  top: 0; right: 0; bottom: 0;
  width: 12px;
  background: #ebe4d8; /* gutter tone step, not a hairline */
  pointer-events: none;
  z-index: 2;
}
.face {
  position: absolute;
  inset: 0;
  background: #f4ebe0;
  overflow: hidden;
  isolation: isolate;
}
.face.front { display: block }
.page-edge {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 4px;
  opacity: 0;
  pointer-events: none;
  z-index: 4;
  background: #cfc4b4;
  transform-origin: center;
}
.sheet.right .page-edge { left: 0 }
.sheet.left .page-edge { right: 0 }
.page-curl {
  position: absolute;
  inset: 0;
  pointer-events: none;
  z-index: 3;
  background: linear-gradient(90deg, transparent 48%, rgba(0,0,0,0.12) 68%, rgba(255,255,255,0.22) 88%, rgba(0,0,0,0.1) 100%); /* taste-skill-ignore */
  opacity: 0;
}
.sheet.left .page-curl {
  background: linear-gradient(270deg, transparent 48%, rgba(0,0,0,0.12) 68%, rgba(255,255,255,0.22) 88%, rgba(0,0,0,0.1) 100%); /* taste-skill-ignore */
}
.sheet.is-dragging {
  z-index: 12;
  box-shadow: -22px 0 48px rgba(0,0,0,0.32); /* taste-skill-ignore */
}
.page .page-inner, .face .page-inner, .sheet.under .page-inner {
  position: relative;
  padding: clamp(1rem, 2.2vw, 1.85rem) clamp(1rem, 2.2vw, 1.85rem) 2.4rem;
  height: 100%;
  overflow: auto;
  font-size: clamp(0.88rem, 1.15vw, 1.05rem);
  line-height: 1.55;
  font-family: "Bricolage Grotesque", Georgia, serif;
  white-space: normal;
  contain: paint;
  /* Warm paper + full warm ink — readable, not harsh black/white */
  color: #1a120c;
  background: #f4ebe0;
}
.page .page-num, .face .page-num {
  position: absolute;
  bottom: 0.75rem;
  right: 1rem;
  font-size: 0.75rem;
  color: #1a120c;
  opacity: 0.55;
  z-index: 2;
}
.page-inner h1, .page-inner h2, .page-inner h3 {
  margin: 0 0 0.55rem;
  letter-spacing: -0.03em;
  line-height: 1.1;
  font-weight: 600;
  color: #1a120c;
}
.page-inner h1 { font-size: clamp(1.25rem, 2.2vw, 1.7rem) }
.page-inner h2 { font-size: clamp(1.05rem, 1.7vw, 1.35rem) }
.page-inner h3 { font-size: clamp(0.95rem, 1.4vw, 1.15rem) }
.page-inner p {
  margin: 0 0 0.65rem;
  max-width: 62ch;
  color: #1a120c;
}
.page-inner ul, .page-inner ol {
  margin: 0 0 0.75rem;
  padding-left: 1.15rem;
  color: #1a120c;
}
.page-inner li { margin: 0.15rem 0; color: #1a120c }
.page-inner strong { font-weight: 650; color: #1a120c }
.page-inner em { font-style: italic; color: #1a120c }
.page-inner a {
  color: #8a3a00;
  text-decoration: underline;
  text-underline-offset: 2px;
}
/* Code: soft paper-dark field + Cascadia Mono */
.page-inner .sr-block, .page-inner .code-block,
.page-inner pre.sr-block, .page-inner pre.code-block {
  margin: 0.55rem 0 0.85rem;
  padding: 0.75rem 0.9rem;
  font-size: 0.82rem;
  line-height: 1.45;
  max-height: 55%;
  overflow: auto;
  background: #524840 !important;
  color: #f5eee6 !important;
  font-family: var(--font-mono);
  font-weight: 400;
  letter-spacing: 0;
}
.page-inner .sr-block code, .page-inner .code-block code {
  color: inherit;
  background: transparent;
  font-family: inherit;
  font-size: inherit;
}
.page-inner .sr-block .tok-kw, .page-inner .code-block .tok-kw { color: #ff9a4a; font-weight: 600 }
.page-inner .sr-block .tok-type, .page-inner .code-block .tok-type { color: #e8b88a }
.page-inner .sr-block .tok-string, .page-inner .code-block .tok-string { color: #a8d4a0 }
.page-inner .sr-block .tok-comment, .page-inner .code-block .tok-comment { color: #a89078 }
.page-inner .sr-block .tok-number, .page-inner .code-block .tok-number { color: #7eb8d4 }
.page-inner .sr-block .tok-op, .page-inner .code-block .tok-op { color: #c4a882 }
.page-inner .sr-block .tok-macro, .page-inner .code-block .tok-macro { color: #e8c46a; font-weight: 600 }
.page-inner .sr-block .tok-mut, .page-inner .code-block .tok-mut { color: #e888a0; font-weight: 600 }
.page-inner .sr-block .tok-vis, .page-inner .code-block .tok-vis { color: #ff9a4a; font-weight: 700 }
/* Inline ``ticks``: visible chip + Cascadia — never plain unstyled */
.page-inner code.sr-inline,
code.sr-inline {
  font-family: var(--font-mono) !important;
  font-size: 0.9em;
  font-weight: 500;
  background: #e4d8c8 !important;
  color: #6e2e00 !important;
  padding: 0.12em 0.38em;
  letter-spacing: -0.01em;
  box-decoration-break: clone;
  -webkit-box-decoration-break: clone;
}
.page-inner code.sr-inline .tok-kw { color: #9a3b00 !important; font-weight: 650 }
.page-inner code.sr-inline .tok-type { color: #6e3500 !important }
.page-inner code.sr-inline .tok-string { color: #0a5a24 !important }
.page-inner code.sr-inline .tok-comment { color: #5a4638 !important }
.page-inner code.sr-inline .tok-number { color: #084e78 !important }
.page-inner code.sr-inline .tok-op { color: #5a4030 !important }
.page-inner code.sr-inline .tok-macro { color: #6e4e00 !important; font-weight: 650 }
.page-inner code.sr-inline .tok-mut { color: #9a0a35 !important; font-weight: 650 }
.page-inner code.sr-inline .tok-vis { color: #9a3b00 !important; font-weight: 650 }
.page-inner code.sr-inline .tok-kw,
.page-inner code.sr-inline .tok-type,
.page-inner code.sr-inline .tok-string,
.page-inner code.sr-inline .tok-comment,
.page-inner code.sr-inline .tok-number,
.page-inner code.sr-inline .tok-op,
.page-inner code.sr-inline .tok-macro,
.page-inner code.sr-inline .tok-mut,
.page-inner code.sr-inline .tok-vis {
  background: transparent !important;
}
/* —— Apple Books–style covers: centered type + quiet diamond motif —— */
.book-cover {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  text-align: center;
  padding: clamp(1.75rem, 4.5vw, 3rem);
  background: #1a120c;
  color: #ff7a1f;
  overflow: hidden;
  z-index: 1;
}
.book-cover.section-cover {
  background: #ff7a1f;
  color: #1a120c;
}
.book-cover .cover-art {
  position: absolute;
  inset: 0;
  pointer-events: none;
  overflow: hidden;
  z-index: 0;
}
/* Plain rotated square — no clip-path / SVG / filter (those mangled prior art) */
.book-cover .cover-diamond {
  position: absolute;
  top: 16%;
  left: 50%;
  width: min(26%, 7rem);
  aspect-ratio: 1;
  margin: 0;
  background: #ff7a1f;
  transform: translate(-50%, 0) rotate(45deg);
  transform-origin: center center;
  opacity: 0.22;
}
.book-cover.section-cover .cover-diamond {
  background: #1a120c;
  top: auto;
  bottom: 14%;
  opacity: 0.14;
}
.book-cover .cover-copy {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  text-align: center;
  max-width: 92%;
  margin: 0 auto;
}
.book-cover h1 {
  margin: 0;
  font-size: clamp(1.55rem, 3.2vw, 2.45rem);
  letter-spacing: -0.045em;
  line-height: 0.95;
  font-weight: 500;
  max-width: 12ch;
  color: inherit;
  text-align: center;
}
.book-cover .cover-deck {
  margin: 1.1rem auto 0;
  max-width: 28ch;
  font-size: clamp(0.95rem, 1.35vw, 1.1rem);
  line-height: 1.35;
  color: inherit;
  text-align: center;
}
.book-cover .cover-by {
  margin: 1.5rem auto 0;
  font-size: 0.95rem;
  color: inherit;
  opacity: 0.9;
  text-align: center;
}
.book-cover .cover-meta {
  position: absolute;
  z-index: 1;
  left: 0;
  right: 0;
  bottom: clamp(1.5rem, 4vw, 2.5rem);
  margin: 0;
  font-size: 0.875rem;
  color: inherit;
  opacity: 0.72;
  letter-spacing: -0.01em;
  text-align: center;
}
.sheet.is-cover .page-num,
.sheet.is-cover .face .page-num { display: none }
.sheet.is-cover .page-inner,
.sheet.is-cover .face .page-inner,
.sheet.under.is-cover .page-inner {
  padding: 0;
  overflow: hidden;
  background: transparent;
  /* Drop contain:paint inheritance so rotated motif is not garbled */
  contain: none;
}
.sheet.is-cover .face {
  background: #1a120c;
  contain: none;
}
.sheet.is-cover.section-face .face,
.sheet.is-cover .face:has(.section-cover) {
  background: #ff7a1f;
  contain: none;
}
/* Legacy class hooks — inert. Spring RAF owns transform; CSS transition fought the snap. */
.page.flipping, .sheet.is-flipping {
  transition: none !important;
}
.reader-progress {
  height: 3px;
  background: #2a2018;
}
.reader-progress > i {
  display: block;
  height: 100%;
  width: 0;
  background: var(--accent-ink);
  transition: width 300ms var(--ease);
}
.reader-hint {
  text-align: center;
  padding: 0.28rem 0.5rem 0.4rem;
  flex: 0 0 auto;
  opacity: 0.65;
  font-size: 0.8rem;
  color: #c4a48a;
  margin: 0;
}

[data-reveal] {
  opacity: 0;
  transform: translateY(24px);
  transition: opacity 700ms var(--ease), transform 700ms var(--ease);
}
[data-reveal].in { opacity: 1; transform: none }
[data-reveal]:nth-child(2) { transition-delay: 70ms }
[data-reveal]:nth-child(3) { transition-delay: 140ms }
[data-reveal]:nth-child(4) { transition-delay: 210ms }
[data-reveal]:nth-child(5) { transition-delay: 280ms }

@media (max-width: 900px) {
  .hero-board, .stats-wrap, .faq-grid, .cta-band, .intro-grid, .faq-bands .faq-band {
    grid-template-columns: 1fr;
  }
  .tile-winner {
    grid-row: auto;
    min-height: 0;
    clip-path: none;
    animation: riseIn 800ms var(--ease) both;
  }
  .tile-winner::before {
    -webkit-mask-size: min(110%, 28rem);
    mask-size: min(110%, 28rem);
    -webkit-mask-position: right -20% bottom -22%;
    mask-position: right -20% bottom -22%;
    opacity: 0.05;
  }
  @keyframes riseIn {
    from { opacity: 0; transform: translateY(28px) }
    to { opacity: 1; transform: none }
  }
  .tile-winner h1 { font-size: clamp(3rem, 18vw, 4.5rem) }
  .tile-cats { min-height: 0 }
  .cat-grid { grid-template-columns: 1fr 1fr }
  .cat-tile { min-height: 5.75rem }
  .cat-tile .cat-mark { font-size: clamp(2.15rem, 11vw, 3.25rem) }
  .lang-line {
    font-size: 14vw;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.2rem;
  }
  .weight-bar { flex-direction: column; min-height: 0 }
  .wseg { min-height: 5.5rem }
  .abar { grid-template-columns: 5.5rem 1fr 4.5rem }
  .site-nav { grid-template-columns: 1fr; font-size: var(--step-small) }
  .site-nav .cta { display: none }
  .footer-mark { height: 22svh }
  .footer-mark span { font-size: 12vw }
  #timings-table { display: none }
  .timing-cards { display: grid }
  .book-spread {
    top: 3%;
    left: 3%;
    width: 94%;
    height: 94%;
  }
  .sheet.left, .sheet.under.left { display: none }
  .sheet.right, .sheet.under.right { left: 0; width: 100% }
  .face .page-inner, .sheet.under .page-inner { font-size: 0.92rem }
  .reader-bar { padding: 0.4rem 0.65rem }
  .reader-hint { padding: 0.25rem; font-size: 0.75rem }
}
@media (max-width: 420px) {
  .cat-grid { grid-template-columns: 1fr }
  .cat-tile:nth-child(5) { grid-column: auto }
  .abar { grid-template-columns: 4.5rem 1fr 3.8rem; font-size: 0.875rem }
}



.weight-poster {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: var(--gap);
  margin-top: 1.5rem;
  min-height: 10rem;
  align-items: end;
}
.weight-line {
  margin: 0;
  padding: 1rem 0.75rem;
  background: #ff7a1f;
  color: #050f16;
  font-size: clamp(1rem, 2.2vw, 1.25rem);
  letter-spacing: -0.02em;
  line-height: 1.15;
}
.weight-line strong {
  display: block;
  font-size: clamp(2rem, 5vw, 3.25rem);
  letter-spacing: -0.05em;
  line-height: 0.9;
  margin-bottom: 0.35rem;
}
@media (max-width: 900px) {
  .weight-poster { grid-template-columns: 1fr 1fr; }
  .weight-line:last-child { grid-column: 1 / -1; }
}


.runtime-board, .pipeline-board {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--gap);
  min-height: 220px;
  padding: 1.25rem;
  background: var(--field);
  color: var(--field-ink);
}
.runtime-board > div, .pipeline-board li {
  padding: 1rem 0.75rem;
  background: #ff7a1f;
  color: #050f16;
  display: flex;
  flex-direction: column;
  gap: 0.35rem;
  min-height: 5.5rem;
}
.runtime-board strong, .pipeline-board span {
  font-size: clamp(1.35rem, 3vw, 1.85rem);
  letter-spacing: -0.03em;
}
.runtime-board em { font-style: normal; opacity: 0.8 }
.pipeline-board { display: block; background: #050f16; color: #ff7a1f }
.pipeline-board ol {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--gap);
}
.pipeline-board li { background: #ff7a1f; color: #050f16; margin: 0 }
.weight-poster { min-height: 220px }
.code-stage { min-height: 220px }
@media (max-width: 900px) {
  .runtime-board, .pipeline-board ol { grid-template-columns: 1fr }
}

/* —— Winner chips —— */
.algo-head {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 0.65rem 1rem;
  margin-bottom: 0.35rem;
}
.algo-head h3, .algo-head h4 { margin: 0 }
.winner-chip {
  display: inline-flex;
  align-items: center;
  padding: 0.28rem 0.7rem;
  background: var(--ink);
  color: var(--accent-ink);
  font-size: 0.9375rem;
  font-weight: 600;
  letter-spacing: -0.02em;
  line-height: 1.2;
}
.abar.is-algo-winner .afill { background: var(--ink) }
.abar.is-algo-winner span { font-weight: 600 }

/* —— Toolchain denser panel —— */
.toolchain-grid {
  display: grid;
  grid-template-columns: 1.2fr 0.8fr;
  gap: var(--gap);
  margin-top: 1.25rem;
}
.toolchain-panel {
  background: var(--field);
  color: var(--field-ink);
  padding: 1.25rem 1.35rem 1.5rem;
}
.toolchain-panel h3 {
  margin: 0 0 1rem;
  font-size: clamp(1.35rem, 2.2vw, 1.75rem);
  letter-spacing: -0.03em;
}
.toolchain-panel .table-wrap { margin: 0 }
.toolchain-panel .data th {
  font-size: 0.875rem;
  text-transform: none;
  letter-spacing: 0;
  opacity: 0.7;
  padding-top: 0.35rem;
  padding-bottom: 0.55rem;
}
.toolchain-panel .data td,
.toolchain-panel .data th {
  padding-left: 0;
  padding-right: 0.75rem;
}
@media (max-width: 900px) {
  .toolchain-grid { grid-template-columns: 1fr }
  .data-stack thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0) }
  .data-stack tr {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.15rem 1rem;
    padding: 0.85rem 0;
    border-bottom: 3px solid color-mix(in srgb, var(--field-ink) 12%, transparent);
  }
  .data-stack td {
    display: block;
    padding: 0;
    border: 0;
  }
  .data-stack td:first-child {
    font-weight: 600;
    grid-column: 1 / -1;
  }
}

/* —— Super syntax highlighting —— */
.sr-block, pre.sr-block, pre.code-block {
  background: #524840;
  color: #f5eee6;
  padding: 1.1rem 1.25rem;
  overflow-x: auto;
  font-size: 0.95rem;
  line-height: 1.45;
  font-family: var(--font-mono);
  font-weight: 400;
  letter-spacing: 0;
}
.sr-block code, code.language-sr, .code-block code {
  font-family: inherit;
  font-size: inherit;
}
code.sr-inline {
  font-family: var(--font-mono) !important;
  background: #e4d8c8 !important;
  color: #6e2e00 !important;
  padding: 0.12em 0.38em;
  font-weight: 500;
}
.tok-kw { color: #ff7a1f; font-weight: 600 }
.tok-type { color: #ffc089 }
.tok-string { color: #9dffa8 }
.tok-comment { color: #9a7a66; font-weight: 400 }
.tok-number { color: #7ec8ff }
.tok-op { color: #d4b896 }
.tok-macro { color: #ffd24a; font-weight: 600 }
.tok-mut { color: #ff5c7a; font-weight: 600 }
.tok-vis { color: #ff7a1f; font-weight: 700 }
.spec-body pre.sr-block { max-width: 100% }
.spec-body code.sr-inline .tok-kw { color: #c45a00 }

/* —— Examples catalogue —— */
.example-nav {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
}
.example-nav a {
  background: #ff7a1f;
  color: #050f16;
  padding: 0.45rem 0.75rem;
  font-weight: 600;
  text-decoration: none;
}
.example-nav a:hover, .example-nav a:focus-visible {
  outline: 3px solid #fff;
  outline-offset: 2px;
}
.examples-stage { display: grid; gap: 2rem }
.example-block .example-meta { margin-bottom: 0.75rem }
.example-kind {
  display: inline-block;
  background: var(--ink);
  color: var(--accent-ink);
  padding: 0.2rem 0.55rem;
  font-size: 0.8125rem;
  font-weight: 600;
  margin-bottom: 0.4rem;
}
.example-block h3 {
  margin: 0.2rem 0;
  font-size: clamp(1.5rem, 3vw, 2.1rem);
  letter-spacing: -0.03em;
}

/* —— Book drag physics —— */
.sheet.hard {
  transition: none;
}
.page-curl { opacity: 0 }

@media (prefers-reduced-motion: reduce) {
  .tile-winner, .afill, [data-reveal], .page.flipping {
    animation: none !important;
    transition: none !important;
    transform: none !important;
    opacity: 1 !important;
    clip-path: none !important;
  }
  /* Keep page turns available; curl transforms are skipped in JS */
  .sheet.is-dragging {
    box-shadow: none !important; /* taste-skill-ignore */
  }
  html { scroll-behavior: auto }
}

.band-ink { background: #050f16 !important; color: #ff7a1f !important }
.section.band-ink h2 { color: #ff7a1f !important }
.section.band-ink .note, .section.band-ink .live-hint, .section.band-ink .chart-note {
  color: #ffb07a !important;
}
.chart-note { max-width: 42ch; margin: 0 0 0.75rem }
#score { position: relative; z-index: 2; background: var(--bg) }
""".strip()


def site_js_shared() -> str:
    return """
(function () {
  const io = new IntersectionObserver((entries) => {
    entries.forEach((e) => {
      if (e.isIntersecting) {
        e.target.classList.add("in");
        const delayKids = e.target.querySelectorAll(":scope > [data-reveal], :scope [data-stagger] > *");
        delayKids.forEach((kid, i) => {
          kid.style.transitionDelay = (i * 70) + "ms";
          kid.classList.add("in");
        });
      }
    });
  }, { threshold: 0.12 });
  document.querySelectorAll("[data-reveal]").forEach((el) => io.observe(el));
})();
(function () {
  document.querySelectorAll("[data-copy-target]").forEach((btn) => {
    const label = btn.textContent || "Copy";
    btn.addEventListener("click", () => {
      const id = btn.getAttribute("data-copy-target");
      const el = id ? document.getElementById(id) : null;
      if (!el) return;
      const text = el.textContent || "";
      const done = () => {
        btn.textContent = "Copied";
        setTimeout(() => { btn.textContent = label; }, 1800);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done).catch(() => {
          window.prompt("Copy install command", text);
        });
      } else {
        window.prompt("Copy install command", text);
      }
    });
  });
})();
""".strip()


def nav_html(active: str) -> str:
    # Pretty URLs match the published GitHub Pages tree (no Language duplicate of Home).
    links = [
        ("./", "Home"),
        ("benchmarks/", "Benchmarks"),
        ("winners/", "Winners"),
        ("charts/", "Charts"),
        ("spec/", "Spec"),
        ("book/", "Book"),
        ("examples/", "Examples"),
        ("install/", "Install"),
    ]
    items = []
    for href, label in links:
        cur = ' aria-current="page"' if active in (href, href.rstrip("/"), href + "index.html") else ""
        items.append(f'<a href="{href}"{cur}>{_esc(label)}</a>')
    items.append(
        '<a href="https://github.com/theHamdiz/super-lang" rel="noopener noreferrer">GitHub</a>'
    )
    items.append(
        '<a href="https://discord.gg/ZvgmreZsuY" rel="noopener noreferrer">Discord</a>'
    )
    items.append(
        '<a href="https://ko-fi.com/thehamdiz" rel="noopener noreferrer">Donate</a>'
    )
    return f"""
<header class="site-nav">
  <a class="mark" href="./">Super</a>
  <nav>{''.join(items)}</nav>
  <a class="cta" href="downloads/SUPER_BOOK.pdf" download>Download book</a>
</header>
""".strip()


def footer_html(mach: dict[str, Any] | None = None, generated: Any = None) -> str:
    # Machine/host lines and raw results.json links stay off the public chrome.
    _ = (mach, generated)
    return """
<footer class="site-footer">
  <div class="footer-links">
    <a href="benchmarks/">Benchmarks</a>
    <a href="winners/">Winners</a>
    <a href="charts/">Charts</a>
    <a href="book/">Book</a>
    <a href="install/">Install</a>
    <a href="https://github.com/theHamdiz/super-lang" rel="noopener noreferrer">GitHub</a>
    <a href="https://discord.gg/ZvgmreZsuY" rel="noopener noreferrer">Discord</a>
    <a href="https://ko-fi.com/thehamdiz" rel="noopener noreferrer">Donate</a>
  </div>
  <div class="footer-mark" aria-hidden="true">
    <span>S</span><span>u</span><span>p</span><span>e</span><span>r</span>
  </div>
  <div class="footer-legal">
    <span>Super language</span>
  </div>
</footer>
""".strip()


SITE_ORIGIN = "https://thehamdiz.github.io/super-lang"
_PAGE_BLURBS: dict[str, str] = {
    "index.html": "Low-boilerplate .sr with SuperAot and SuperJit — scoped stacks, UFCS, @ mutation, and Pod.toml.",
    "benchmarks.html": "Stage-1 equal-algorithm suite — SuperAot leads the composite board.",
    "winners.html": "Category winners from the equal-algorithm Stage-1 suite.",
    "charts.html": "Live Stage-1 composite charts — filter by Speed, Memory, Reliability, Cycle, LoC.",
    "language.html": "Low-boilerplate .sr with SuperAot and SuperJit — scoped stacks, UFCS, @ mutation, and Pod.toml.",
    "install.html": "Install Super 0.1.0 with a one-liner into ~/.super — CLI, stdlib, LSP, and MCP outside ~/.cargo.",
    "examples.html": "Space shooter, task API, and log analyzer — runnable .sr projects with first-class tests.",
    "book.html": "The Super Programming Language book — read online or download the PDF.",
    "spec.html": "Normative Super language surface: grammar, types, stdlib contracts, diagnostics.",
}


def head(title: str, *, extra: str = "", page: str | None = None, description: str | None = None) -> str:
    """Document head with favicon + SEO/GEO + Open Graph / Twitter share cards."""
    page_name = page or "index.html"
    if page_name.startswith("Super"):
        # callers sometimes pass the title accidentally; keep index as default
        page_name = "index.html"
    desc = description or _PAGE_BLURBS.get(page_name, "Super — ultra-fast .sr language for APIs, games, and agents.")
    canonical = f"{SITE_ORIGIN}/{page_name}"
    og_image = f"{SITE_ORIGIN}/assets/og-image.png"
    ld = {
        "@context": "https://schema.org",
        "@type": "SoftwareApplication",
        "name": "Super",
        "applicationCategory": "DeveloperApplication",
        "operatingSystem": "Linux, macOS, FreeBSD, Windows",
        "softwareVersion": "0.1.0",
        "license": "https://opensource.org/licenses/MIT",
        "url": f"{SITE_ORIGIN}/",
        "description": desc,
        "downloadUrl": f"{SITE_ORIGIN}/install.html",
        "author": {"@type": "Person", "name": "Ahmad Hamdi"},
    }
    ld_json = json.dumps(ld, ensure_ascii=False)
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<link rel="canonical" href="{_esc(canonical)}"/>
<link rel="icon" href="assets/favicon.ico" sizes="any"/>
<link rel="icon" type="image/png" sizes="32x32" href="assets/favicon-32.png"/>
<link rel="icon" type="image/png" href="assets/icons/super.png"/>
<link rel="apple-touch-icon" href="assets/icons/super.png"/>
<meta name="description" content="{_esc(desc)}"/>
<meta name="theme-color" content="#ff7a1f"/>
<meta property="og:type" content="website"/>
<meta property="og:site_name" content="Super"/>
<meta property="og:locale" content="en_US"/>
<meta property="og:title" content="{_esc(title)}"/>
<meta property="og:description" content="{_esc(desc)}"/>
<meta property="og:url" content="{_esc(canonical)}"/>
<meta property="og:image" content="{_esc(og_image)}"/>
<meta property="og:image:width" content="1200"/>
<meta property="og:image:height" content="630"/>
<meta property="og:image:alt" content="Super language"/>
<meta name="twitter:card" content="summary_large_image"/>
<meta name="twitter:title" content="{_esc(title)}"/>
<meta name="twitter:description" content="{_esc(desc)}"/>
<meta name="twitter:image" content="{_esc(og_image)}"/>
<script type="application/ld+json">{ld_json}</script>
<title>{_esc(title)}</title>
<link rel="preconnect" href="https://fonts.googleapis.com"/>
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin/>
<link href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,400;12..96,500;12..96,600&display=swap" rel="stylesheet"/>
<link rel="stylesheet" href="site.css"/>
<script src="super-highlight.js" defer></script>
{extra}
</head>
<body>
"""


def faq_items() -> list[tuple[str, str]]:
    return [
        (
            "What is the timing protocol?",
            "Versioned protocol (see Protocol version in the suite report): discard 1–2 "
            "warm-up runs per language, then N timed runs (default 10) interleaved "
            "round-robin across languages. Primary wall is the median (or trimmed "
            "mean). Every figure ships with stdev and a CI; overlapping CIs are "
            "within noise — not a hard ranking.",
        ),
        (
            "What is Cycle?",
            "Compile wall (zero for interpreters and SuperJit) plus the first "
            "checksummed verify run. It approximates an agent edit→test loop.",
        ),
        (
            "What is Reliability?",
            "Share of gated algorithms that finish with a matching CHECKSUM. "
            "Incomplete ports cannot hide behind a selective geo-mean.",
        ),
        (
            "How does ratio-to-best work?",
            "For lower-is-better metrics, score = 100 × best / value. "
            "For Reliability, score = 100 × rate / best rate. Floor is 0.01.",
        ),
        (
            "How do failures affect Speed?",
            "A checksum or compile failure injects 2× the slowest successful "
            "primary wall on that algorithm (or the suite timeout) into the geo-mean.",
        ),
        (
            "What does LoC measure?",
            "Non-blank, non-comment source lines for the primary file of each "
            "language. SuperAot and SuperJit share algo.sr.",
        ),
    ]



def _wall_reading(st: dict[str, Any] | None) -> float | None:
    """Prefer protocol primary (median/trimmed), then median, then mean."""
    if not st:
        return None
    for key in ("primary", "median", "trimmed_mean", "mean"):
        v = st.get(key)
        if isinstance(v, (int, float)) and float(v) > 0:
            return float(v)
    return None


def algo_overall_winner(algo: dict[str, Any]) -> str | None:
    """Per-algorithm chip / `is-algo-winner`: fastest primary wall.

    Must match timing-card `is-best` / table `is-best-cell`. Suite-level
    composite (speed/memory/reliability/cycle/loc) is separate — do **not**
    reintroduce a per-algo mini-composite here. Cycle (compile + first run)
    used to crown SuperJit on algo 15 while Zig/Rust/C won wall time, which
    mismatched the wall-length bars next to the chip.
    """
    ok = [t for t in (algo.get("targets") or []) if t.get("status") == "ok"]
    best_name = None
    best_wall: float | None = None
    for t in ok:
        wall = _wall_reading(t.get("stats"))
        if wall is None or wall <= 0:
            continue
        if best_wall is None or wall < best_wall:
            best_wall = wall
            best_name = t.get("target")
    return best_name


def build_timings_matrix(algos: list[dict[str, Any]]) -> tuple[list[str], list[dict[str, Any]]]:
    """Return (ordered targets, rows of {name, means: {target: primary wall}})."""
    targets: list[str] = []
    seen: set[str] = set()
    for algo in algos:
        for t in algo.get("targets") or []:
            name = t.get("target")
            if name and name not in seen:
                seen.add(name)
                targets.append(name)
    rows = []
    for algo in algos:
        means: dict[str, Optional[float]] = {}
        for t in algo.get("targets") or []:
            st = t.get("status")
            mean = _wall_reading(t.get("stats")) if st == "ok" else None
            means[t.get("target")] = mean
        rows.append({"name": algo.get("name", ""), "means": means})
    return targets, rows


def _inline_md(text: str) -> str:
    """Single-escape markdown inline: stash ticks first so code is never double-escaped.

    Prior bug: ``_esc(text)`` (quote=True) then ``_esc(m.group(1))`` on already-escaped
    tick bodies produced literal ``&amp;quot;`` in the DOM (visible as ``&quot;``).
    """
    codes: list[str] = []

    def _stash(m: re.Match[str]) -> str:
        codes.append(m.group(1))
        return f"\x00C{len(codes) - 1}\x00"

    raw = re.sub(r"`([^`\n]+)`", _stash, text)
    s = _esc_text(raw)
    s = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r'<a href="\2">\1</a>', s)
    s = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", s)
    s = re.sub(r"(^|[^*])\*([^*]+)\*(?!\*)", r"\1<em>\2</em>", s)
    for i, code in enumerate(codes):
        s = s.replace(
            f"\x00C{i}\x00",
            f'<code class="sr-inline">{_esc_text(code)}</code>',
        )
    return s


def _code_lang_class(lang: str) -> tuple[str, str]:
    lang = (lang or "sr").strip().lower()
    if lang in {"", "super", "sr"}:
        return "sr-block", "language-sr"
    if lang in {"bash", "sh", "shell", "zsh"}:
        return "code-block", "language-bash"
    return "code-block", f"language-{lang}"


def _estimate_block_weight(kind: str, text: str) -> int:
    lines = max(1, text.count("\n") + 1)
    if kind == "code":
        return 4 + lines * 2
    if kind.startswith("h"):
        return 3 + max(1, len(text) // 48)
    if kind == "list":
        return 2 + lines * 2
    return 2 + max(1, len(text) // 70)


def parse_markdown_blocks(md: str) -> list[dict[str, str]]:
    """Split markdown into coarse blocks for book pagination."""
    blocks: list[dict[str, str]] = []
    lines = md.splitlines()
    i = 0
    para: list[str] = []
    list_items: list[str] = []

    def flush_para() -> None:
        nonlocal para
        if not para:
            return
        text = " ".join(p.strip() for p in para if p.strip())
        if text:
            blocks.append({"kind": "p", "text": text})
        para = []

    def flush_list() -> None:
        nonlocal list_items
        if not list_items:
            return
        blocks.append({"kind": "list", "text": "\n".join(list_items)})
        list_items = []

    while i < len(lines):
        line = lines[i]
        fence = re.match(r"^```(\w+)?\s*$", line)
        if fence:
            flush_para()
            flush_list()
            lang = fence.group(1) or "sr"
            i += 1
            buf: list[str] = []
            while i < len(lines) and not re.match(r"^```", lines[i]):
                buf.append(lines[i])
                i += 1
            if i < len(lines):
                i += 1
            blocks.append({"kind": "code", "lang": lang, "text": "\n".join(buf)})
            continue
        if re.match(r"^#{1,3}\s+", line):
            flush_para()
            flush_list()
            level = len(re.match(r"^(#{1,3})", line).group(1))
            blocks.append({"kind": f"h{level}", "text": re.sub(r"^#{1,3}\s+", "", line).strip()})
            i += 1
            continue
        if re.match(r"^\s*[-*•]\s+", line) or re.match(r"^\s*\d+\.\s+", line):
            flush_para()
            list_items.append(re.sub(r"^\s*(?:[-*•]|\d+\.)\s+", "", line).strip())
            i += 1
            continue
        if not line.strip():
            flush_para()
            flush_list()
            i += 1
            continue
        flush_list()
        para.append(line)
        i += 1
    flush_para()
    flush_list()
    return blocks


def render_md_block(block: dict[str, str]) -> str:
    kind = block.get("kind", "p")
    text = block.get("text", "")
    if kind == "code":
        pre_cls, code_cls = _code_lang_class(block.get("lang", "sr"))
        # quote=False so "hello" stays a real quote in HTML text (not &quot;)
        return f'<pre class="{pre_cls}"><code class="{code_cls}">{_esc_text(text)}</code></pre>'
    if kind.startswith("h") and kind[1:].isdigit():
        level = int(kind[1:])
        return f"<h{level}>{_inline_md(text)}</h{level}>"
    if kind == "list":
        items = "".join(f"<li>{_inline_md(it)}</li>" for it in text.split("\n") if it.strip())
        return f"<ul>{items}</ul>"
    return f"<p>{_inline_md(text)}</p>"


def render_front_cover(
    *,
    title: str = "The Super Programming Language",
    deck: str = "A token-compact language for coding agents. Goose-style stacks. Native speed.",
    author: str = "Ahmad Hamdi",
    version: str = "0.1.0",
) -> dict[str, Any]:
    """Designed Apple Books–style front cover (not a markdown dump)."""
    html = (
        '<article class="book-cover no-hl" data-cover="front">'
        '<div class="cover-art" aria-hidden="true"><span class="cover-diamond"></span></div>'
        '<div class="cover-copy">'
        f"<h1>{_esc(title)}</h1>"
        f'<p class="cover-deck">{_esc(deck)}</p>'
        f'<p class="cover-by">{_esc(author)}</p>'
        "</div>"
        f'<p class="cover-meta">Version {_esc(version)}</p>'
        "</article>"
    )
    return {
        "n": 0,
        "kind": "cover",
        "title": title,
        "html": html,
        "lines": [title, deck, author, f"Version {version}"],
    }


def render_section_cover(title: str, deck: str = "") -> dict[str, Any]:
    """Chapter / part cover with intentional visual field."""
    deck_html = f'<p class="cover-deck">{_esc(deck)}</p>' if deck.strip() else ""
    html = (
        '<article class="book-cover section-cover no-hl" data-cover="section">'
        '<div class="cover-art" aria-hidden="true"><span class="cover-diamond"></span></div>'
        '<div class="cover-copy">'
        f"<h1>{_esc(title)}</h1>"
        f"{deck_html}"
        "</div>"
        "</article>"
    )
    return {
        "n": 0,
        "kind": "section",
        "title": title,
        "html": html,
        "lines": [title] + ([deck] if deck.strip() else []),
    }


def _is_major_section_heading(kind: str, text: str) -> bool:
    if kind not in {"h1", "h2"}:
        return False
    t = text.strip()
    if kind == "h1":
        return True
    # h2 chapter markers: "1. Why Super exists", "Part II", "Chapter 3"
    return bool(
        re.match(r"^(Part|Chapter|Appendix)\b", t, re.I)
        or re.match(r"^\d+[\.\)]\s+\S", t)
    )


def _section_deck_from_blocks(blocks: list[dict[str, str]], start: int) -> str:
    """Take the first short paragraph after a heading as the section deck."""
    for j in range(start + 1, min(start + 6, len(blocks))):
        b = blocks[j]
        if b.get("kind") == "p":
            text = re.sub(r"\s+", " ", b.get("text", "")).strip()
            # Strip markdown emphasis markers for the deck line
            text = re.sub(r"[`*_]", "", text)
            if len(text) > 140:
                text = text[:137].rstrip() + "…"
            return text
        if b.get("kind", "").startswith("h"):
            break
    return ""


def paginate_markdown_blocks(
    blocks: list[dict[str, str]],
    *,
    max_pages: int | None = None,
    page_budget: int = 42,
    include_covers: bool = True,
) -> list[dict[str, Any]]:
    """Pack markdown blocks into flip-reader pages with HTML + section covers.

    ``max_pages=None`` keeps the full book (no web abridgment).
    """
    pages: list[dict[str, Any]] = []
    cur_html: list[str] = []
    cur_lines: list[str] = []
    weight = 0
    saw_front_title = False
    limit = max_pages if max_pages is not None else 10_000

    def push_page(*, kind: str = "body") -> None:
        nonlocal cur_html, cur_lines, weight
        if not cur_html:
            return
        pages.append(
            {
                "n": len(pages) + 1,
                "kind": kind,
                "html": "\n".join(cur_html),
                "lines": list(cur_lines),
            }
        )
        cur_html = []
        cur_lines = []
        weight = 0

    if include_covers:
        pages.append(render_front_cover())

    for i, block in enumerate(blocks):
        if len(pages) >= limit:
            break
        kind = block.get("kind", "p")
        text = block.get("text", "")

        # Designed section covers for major headings (skip the book title h1 once)
        if include_covers and _is_major_section_heading(kind, text):
            if kind == "h1" and not saw_front_title:
                # First h1 is usually the book title — absorb into front cover, skip dump
                saw_front_title = True
                if re.search(r"programming in super|super programming", text, re.I):
                    continue
            push_page()
            if len(pages) >= limit:
                break
            deck = _section_deck_from_blocks(blocks, i)
            pages.append(render_section_cover(text, deck))
            # Heading still appears once on the following content page
            cur_html.append(render_md_block(block))
            cur_lines.append(text)
            weight = _estimate_block_weight(kind, text)
            continue

        w = _estimate_block_weight(kind, text)
        if (
            cur_html
            and kind.startswith("h")
            and weight >= page_budget * 0.45
        ):
            push_page()
        elif cur_html and weight + w > page_budget:
            push_page()
        if len(pages) >= limit:
            break
        cur_html.append(render_md_block(block))
        if kind == "code":
            lang = block.get("lang", "sr")
            cur_lines.append(f"```{lang}")
            cur_lines.extend(text.splitlines() or [""])
            cur_lines.append("```")
        elif kind == "list":
            for it in text.split("\n"):
                cur_lines.append(f"• {it}")
        elif kind.startswith("h"):
            cur_lines.append(text)
        else:
            cur_lines.append(text)
        weight += w
        if weight >= page_budget:
            push_page()
    push_page()
    if not pages:
        pages = [{"n": 1, "kind": "body", "html": "<p>Empty book.</p>", "lines": ["Empty book."]}]
    # Renumber for the reader chrome
    for i, p in enumerate(pages):
        p["n"] = i + 1
    return pages[:limit]


def _detect_pdf_section_title(lines: list[str]) -> str | None:
    """Heuristic: short title-ish first content line ⇒ section cover."""
    nonempty = [ln.strip() for ln in lines if ln.strip()]
    if not nonempty:
        return None
    head = nonempty[0]
    # Strip common running headers
    if re.search(r"super\s+0\.\d|programming in super|contents", head, re.I):
        if len(nonempty) > 1:
            head = nonempty[1]
        else:
            return None
    if len(head) < 4 or len(head) > 72:
        return None
    if re.match(r"^(Part|Chapter|Appendix)\b", head, re.I):
        return head
    if re.match(r"^\d+[\.\)]\s+[A-Z]", head):
        return head
    # Very short centered-looking titles (few words, title case)
    words = head.split()
    if 2 <= len(words) <= 10 and head[0].isupper() and not head.endswith("."):
        # Avoid ordinary sentences
        if sum(1 for w in words if w[:1].isupper()) >= max(2, len(words) // 2):
            return head
    return None


def decorate_pages_with_covers(
    pages: list[dict[str, Any]],
    *,
    max_pages: int | None = None,
    title: str = "The Super Programming Language",
    author: str = "Ahmad Hamdi",
    version: str = "0.1.0",
) -> list[dict[str, Any]]:
    """Prepend front cover + insert section covers for PDF / plain page lists."""
    limit = max_pages if max_pages is not None else 10_000
    out: list[dict[str, Any]] = [render_front_cover(title=title, author=author, version=version)]
    seen_titles: set[str] = set()
    for raw in pages:
        if len(out) >= limit:
            break
        lines = list(raw.get("lines") or [])
        sec = _detect_pdf_section_title(lines)
        if sec and sec not in seen_titles:
            seen_titles.add(sec)
            deck = ""
            for ln in lines[1:6]:
                t = ln.strip()
                if t and t != sec and len(t) > 20:
                    deck = t[:140] + ("…" if len(t) > 140 else "")
                    break
            out.append(render_section_cover(sec, deck))
            if len(out) >= limit:
                break
        body = dict(raw)
        body.setdefault("kind", "body")
        if not body.get("html") and lines:
            if sec:
                body["html"] = ""
                body["lines"] = [sec] + [ln for ln in lines if ln.strip() != sec]
            else:
                body["html"] = ""
        out.append(body)
    for i, p in enumerate(out):
        p["n"] = i + 1
    return out[:limit]


def sync_book_mdx_from_md() -> Path | None:
    """Keep BOOK.mdx as extensive as BOOK.md (MDX frontmatter + brace-safe prose)."""
    md = REPO / "BOOK.md"
    mdx = REPO / "BOOK.mdx"
    if not md.is_file():
        return None
    body = md.read_text(encoding="utf-8", errors="replace")
    # Escape bare `{` / `}` outside fenced code for MDX parsers
    out_lines: list[str] = []
    in_fence = False
    for ln in body.splitlines():
        if ln.strip().startswith("```"):
            in_fence = not in_fence
            out_lines.append(ln)
            continue
        if in_fence:
            out_lines.append(ln)
        else:
            # Leave markdown link/image syntax alone; escape remaining braces
            safe = ln.replace("{", "&#123;").replace("}", "&#125;")
            out_lines.append(safe)
    front = (
        "---\n"
        "title: Programming in Super\n"
        'version: "0.1.0"\n'
        "---\n\n"
        "This MDX edition carries the complete text of [BOOK.md](./BOOK.md). "
        "Brace characters in prose are HTML-escaped for MDX renderers; "
        "fenced code blocks are unchanged.\n\n"
    )
    mdx.write_text(front + "\n".join(out_lines) + "\n", encoding="utf-8")
    return mdx


def build_book_pages_from_markdown(
    md_path: Path,
    *,
    max_pages: int | None = None,
) -> dict[str, Any] | None:
    """Build rich flip-reader pages from BOOK.md / .mdx / spec markdown (full text)."""
    if not md_path.is_file():
        return None
    text = md_path.read_text(encoding="utf-8", errors="replace")
    # Drop MDX frontmatter / import noise
    if md_path.suffix.lower() == ".mdx":
        if text.startswith("---"):
            end = text.find("\n---", 3)
            if end != -1:
                text = text[end + 4 :].lstrip("\n")
        cleaned: list[str] = []
        for ln in text.splitlines():
            if re.match(r"^\s*(import|export)\s", ln):
                continue
            # Unescape MDX brace entities for the reader
            cleaned.append(ln.replace("&#123;", "{").replace("&#125;", "}"))
        text = "\n".join(cleaned)
    # No line-count abridgment — full book parity with the PDF edition
    blocks = parse_markdown_blocks(text)
    pages = paginate_markdown_blocks(blocks, max_pages=max_pages, include_covers=True)
    return {
        "title": "The Super Programming Language",
        "author": "Ahmad Hamdi",
        "version": "0.1.0",
        "source": str(md_path.name),
        "pages": pages,
    }


def extract_book_pages_from_pdf(pdf: Path, *, max_pages: int | None = None) -> list[dict[str, Any]]:
    """Raw pdftotext page extraction (no covers yet). ``None`` = all pages."""
    pages: list[dict[str, Any]] = []
    if not pdf.is_file():
        return pages
    # Discover page count when unbounded
    total = max_pages or 0
    if max_pages is None:
        try:
            info = subprocess.run(
                ["pdfinfo", str(pdf)], capture_output=True, text=True, check=False
            )
            for line in (info.stdout or "").splitlines():
                if line.startswith("Pages:"):
                    total = int(line.split(":", 1)[1].strip())
                    break
        except (FileNotFoundError, ValueError):
            total = 500
    limit = total or 500
    for i in range(1, limit + 1):
        try:
            r = subprocess.run(
                ["pdftotext", "-f", str(i), "-l", str(i), "-layout", str(pdf), "-"],
                capture_output=True,
                text=True,
                check=False,
            )
        except FileNotFoundError:
            break
        text = (r.stdout or "").replace("\x0c", "").strip()
        if not text and i > 1 and max_pages is None:
            break
        lines = [ln.rstrip() for ln in text.splitlines()]
        while lines and not lines[-1].strip():
            lines.pop()
        pages.append({"n": i, "kind": "body", "lines": lines, "html": ""})
    return pages


def extract_book_pages(pdf: Path, *, max_pages: int | None = None) -> dict[str, Any]:
    """Build flip-reader pages: prefer full BOOK.md/.mdx, else full PDF with covers."""
    # Keep MDX in lockstep with MD before building
    sync_book_mdx_from_md()
    for candidate in (
        REPO / "BOOK.md",
        REPO / "BOOK.mdx",
        REPO / "book" / "parts" / "part1_2.md",
        REPO / "SPECIFICATION.md",
    ):
        data = build_book_pages_from_markdown(candidate, max_pages=max_pages)
        if data and data.get("pages"):
            return data

    raw = extract_book_pages_from_pdf(pdf, max_pages=max_pages)
    if not raw:
        return {
            "title": "The Super Programming Language",
            "author": "Ahmad Hamdi",
            "version": "0.1.0",
            "pages": [
                render_front_cover(deck="Book source not found."),
            ],
        }
    pages = decorate_pages_with_covers(raw, max_pages=max_pages)
    return {
        "title": "The Super Programming Language",
        "author": "Ahmad Hamdi",
        "version": "0.1.0",
        "source": pdf.name if pdf.is_file() else "pdf",
        "pages": pages,
    }


def book_flip_js() -> str:
    asset = SITE_ASSETS / "book-flip.js"
    if asset.is_file():
        return asset.read_text(encoding="utf-8")
    raise FileNotFoundError(f"missing book flip asset: {asset}")


def super_highlight_js() -> str:
    asset = SITE_ASSETS / "super-highlight.js"
    if asset.is_file():
        return asset.read_text(encoding="utf-8")
    raise FileNotFoundError(f"missing highlight asset: {asset}")


def benchmarks_js() -> str:
    return r"""
(function () {
  const data = JSON.parse(document.getElementById("report-data").textContent);
  const order = ["speed", "memory", "reliability", "cycle", "loc"];
  const labels = {
    speed: "Speed × 0.40",
    memory: "Memory × 0.25",
    reliability: "Reliability × 0.15",
    cycle: "Cycle × 0.10",
    loc: "LoC × 0.10"
  };
  const axisNames = {
    speed: "Speed", memory: "Memory", reliability: "Reliability",
    cycle: "Cycle", loc: "LoC"
  };
  let activeLang = data.winner;
  let activeAxis = "score";

  const body = document.getElementById("receipt-body");
  const sum = document.getElementById("receipt-sum");
  const totalEl = document.getElementById("live-total");
  const langEl = document.getElementById("live-lang");
  const axisLive = document.getElementById("axis-live");
  const axisMeter = document.getElementById("axis-meter-fill");
  const chartHost = document.getElementById("rank-chart");

  function ranked() {
    return Object.keys(data.composite || {}).sort(
      (a, b) => data.composite[b].score - data.composite[a].score
    );
  }

  function paintReceipt(lang) {
    const c = data.composite[lang];
    if (!c || !body) return;
    body.innerHTML = order.map((k) => {
      const axis = c[k];
      const contrib = (c.contrib && c.contrib[k]) != null
        ? c.contrib[k]
        : (axis * (data.weights[k] || 0));
      return `<div class="receipt-row"><span>${labels[k]} (${axis.toFixed(1)})</span><span>${Number(contrib).toFixed(2)}</span></div>`;
    }).join("");
    if (sum) sum.textContent = Number(c.score).toFixed(2);
    if (totalEl) totalEl.textContent = Number(c.score).toFixed(1);
    const dn = (data.display && data.display[lang]) || lang;
    if (langEl) langEl.textContent = dn;
  }

  function paintAxisTile(axis) {
    if (!axisLive) return;
    if (axis === "score") {
      const w = data.display[data.winner] || data.winner;
      const sc = data.composite[data.winner];
      axisLive.textContent = w + " leads at " + (sc ? sc.score.toFixed(1) : "—");
      if (axisMeter) axisMeter.style.width = (sc ? Math.min(100, sc.score) : 0) + "%";
      return;
    }
    const winnerKey = (data.category && data.category[axis]) || null;
    const dn = winnerKey ? ((data.display && data.display[winnerKey]) || winnerKey) : "—";
    const c = winnerKey && data.composite[winnerKey];
    const val = c ? c[axis] : 0;
    axisLive.textContent = axisNames[axis] + " · " + dn + " · " + Number(val).toFixed(1);
    if (axisMeter) axisMeter.style.width = Math.min(100, Number(val) || 0) + "%";
  }

  function drawChart() {
    if (!chartHost) return;
    const langs = ranked();
    const h = Math.max(280, langs.length * 36 + 24);
    const w = 720;
    const labelW = 110;
    const valW = 48;
    const trackX = labelW;
    const trackW = w - labelW - valW - 16;
    let maxV = 1;
    langs.forEach((id) => {
      const c = data.composite[id];
      const v = activeAxis === "score" ? c.score : (c[activeAxis] || 0);
      if (v > maxV) maxV = v;
    });
    const rows = langs.map((id, i) => {
      const c = data.composite[id];
      const v = activeAxis === "score" ? c.score : (c[activeAxis] || 0);
      const y = 18 + i * 36;
      const bw = trackW * (v / maxV);
      const dn = (data.display && data.display[id]) || id;
      const cls = [
        "bar-fg",
        id === activeLang ? "is-winner" : "",
        activeAxis !== "score" && data.category && data.category[activeAxis] === id ? "is-winner" : "",
      ].filter(Boolean).join(" ");
      return `
        <g class="rank-row-svg" data-lang="${id}">
          <text x="0" y="${y + 12}">${dn}</text>
          <rect class="bar-bg" x="${trackX}" y="${y}" width="${trackW}" height="18"></rect>
          <rect class="${cls}" x="${trackX}" y="${y}" width="0" height="18" data-w="${bw.toFixed(1)}"></rect>
          <text x="${trackX + trackW + 8}" y="${y + 13}">${v.toFixed(1)}</text>
          <rect class="bar-hit" x="0" y="${y - 6}" width="${w}" height="30" data-lang="${id}" tabindex="0" role="button" aria-label="${dn}"></rect>
        </g>`;
    }).join("");
    chartHost.innerHTML = `<svg class="rank-svg" viewBox="0 0 ${w} ${h}" role="img" aria-label="Composite ranking chart" data-ui>${rows}</svg>`;
    requestAnimationFrame(() => {
      chartHost.querySelectorAll(".bar-fg").forEach((el) => {
        el.setAttribute("width", el.getAttribute("data-w") || "0");
      });
    });
    chartHost.querySelectorAll(".bar-hit").forEach((el) => {
      const go = () => selectLang(el.getAttribute("data-lang"));
      el.addEventListener("click", go);
      el.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); go(); }
      });
    });
  }

  function drawRadar(lang) {
    const host = document.getElementById("radar-chart");
    if (!host) return;
    const c = data.composite[lang];
    if (!c) return;
    const cx = 140, cy = 140, R = 100;
    const pts = order.map((k, i) => {
      const ang = (-Math.PI / 2) + (i / order.length) * Math.PI * 2;
      const r = R * Math.min(1, (c[k] || 0) / 100);
      return [cx + Math.cos(ang) * r, cy + Math.sin(ang) * r];
    });
    const axes = order.map((k, i) => {
      const ang = (-Math.PI / 2) + (i / order.length) * Math.PI * 2;
      const x2 = cx + Math.cos(ang) * R;
      const y2 = cy + Math.sin(ang) * R;
      const lx = cx + Math.cos(ang) * (R + 18);
      const ly = cy + Math.sin(ang) * (R + 18);
      return `<line class="axis" x1="${cx}" y1="${cy}" x2="${x2}" y2="${y2}"></line>
        <text x="${lx}" y="${ly}" text-anchor="middle" dominant-baseline="middle">${axisNames[k]}</text>`;
    }).join("");
    const poly = pts.map((p) => p.join(",")).join(" ");
    host.innerHTML = `<svg class="radar-svg" viewBox="0 0 280 280" data-ui aria-label="Axis radar for selected language">
      <circle class="axis" cx="${cx}" cy="${cy}" r="${R}"></circle>
      <circle class="axis" cx="${cx}" cy="${cy}" r="${R * 0.66}"></circle>
      <circle class="axis" cx="${cx}" cy="${cy}" r="${R * 0.33}"></circle>
      ${axes}
      <polygon class="poly" points="${poly}"></polygon>
    </svg>`;
  }

  function selectLang(lang) {
    if (!data.composite[lang]) return;
    activeLang = lang;
    paintReceipt(lang);
    drawRadar(lang);
    drawChart();
    document.querySelectorAll(".lang-line, .pick").forEach((el) => {
      el.classList.toggle("is-active", el.dataset.lang === lang);
    });
  }

  function selectAxis(axis) {
    activeAxis = axis;
    paintAxisTile(axis);
    drawChart();
    document.querySelectorAll(".cat-tile").forEach((el) => {
      el.classList.toggle("is-active", el.dataset.axis === axis);
    });
    document.querySelectorAll(".chart-toolbar button").forEach((el) => {
      el.classList.toggle("is-active", el.dataset.axis === axis);
    });
    // jump language to category winner when filtering an axis
    if (axis !== "score" && data.category && data.category[axis]) {
      selectLang(data.category[axis]);
    }
  }

  document.querySelectorAll(".lang-line, .pick").forEach((el) => {
    el.addEventListener("click", () => selectLang(el.dataset.lang));
  });
  document.querySelectorAll(".cat-tile").forEach((el) => {
    el.addEventListener("click", () => selectAxis(el.dataset.axis));
  });
  document.querySelectorAll(".chart-toolbar button").forEach((el) => {
    el.addEventListener("click", () => selectAxis(el.dataset.axis));
  });

  // FAQ
  const faqAnswer = document.getElementById("faq-answer");
  document.querySelectorAll("[data-faq]").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll("[data-faq]").forEach((b) => b.classList.remove("is-active"));
      btn.classList.add("is-active");
      const i = Number(btn.dataset.faq);
      if (faqAnswer && data.faq[i]) faqAnswer.textContent = data.faq[i];
    });
  });

  // Matrix language filter (card + table column priority)
  const filterHost = document.getElementById("matrix-filter");
  if (filterHost) {
    const langs = ranked();
    filterHost.innerHTML = `<button type="button" class="is-active" data-show="all">All</button>` +
      langs.map((id) => `<button type="button" data-show="${id}">${(data.display && data.display[id]) || id}</button>`).join("");
    filterHost.querySelectorAll("button").forEach((btn) => {
      btn.addEventListener("click", () => {
        filterHost.querySelectorAll("button").forEach((b) => b.classList.remove("is-active"));
        btn.classList.add("is-active");
        const show = btn.dataset.show;
        document.querySelectorAll(".timing-card [data-lang-row]").forEach((row) => {
          row.hidden = show !== "all" && row.dataset.langRow !== show;
        });
        document.querySelectorAll("#timings-table [data-col]").forEach((cell) => {
          if (show === "all") { cell.hidden = false; return; }
          const col = cell.dataset.col;
          cell.hidden = col !== "algo" && col !== show;
        });
      });
    });
  }

  // Canvas race — moving picture of live scores
  const canvas = document.getElementById("score-race");
  if (canvas && canvas.getContext) {
    const ctx = canvas.getContext("2d");
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const cssW = canvas.clientWidth || 640;
    const cssH = 288;
    canvas.width = cssW * dpr;
    canvas.height = cssH * dpr;
    ctx.scale(dpr, dpr);
    const langs = ranked();
    const max = Math.max(...langs.map((id) => data.composite[id].score), 1);
    let t0 = null;
    function frame(ts) {
      if (t0 == null) t0 = ts;
      const u = Math.min(1, (ts - t0) / 1600);
      const ease = 1 - Math.pow(1 - u, 3);
      ctx.clearRect(0, 0, cssW, cssH);
      ctx.fillStyle = "#050f16";
      ctx.fillRect(0, 0, cssW, cssH);
      langs.forEach((id, i) => {
        const y = 24 + i * 32;
        const sc = data.composite[id].score;
        const w = (cssW - 140) * (sc / max) * ease;
        ctx.fillStyle = id === activeLang ? "#ffffff" : "#ff7a1f";
        ctx.fillRect(120, y, w, 16);
        ctx.fillStyle = "#ffb07a";
        ctx.font = "500 14px Bricolage Grotesque, sans-serif";
        ctx.fillText((data.display && data.display[id]) || id, 8, y + 13);
        ctx.fillStyle = "#ffffff";
        ctx.fillText(sc.toFixed(1), 126 + w, y + 13);
      });
      if (u < 1) requestAnimationFrame(frame);
      else {
        // gentle pulse on winner bar
        let p = 0;
        function pulse() {
          p += 0.02;
          const pulseW = 4 + Math.sin(p) * 3;
          const id = langs[0];
          const sc = data.composite[id].score;
          const y = 24;
          const w = (cssW - 140) * (sc / max);
          ctx.fillStyle = "#ffffff";
          ctx.fillRect(120 + w, y, pulseW, 16);
          requestAnimationFrame(pulse);
        }
        if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
          requestAnimationFrame(pulse);
        }
      }
    }
    requestAnimationFrame(frame);
  }

  selectLang(activeLang);
  selectAxis("score");
  const axisParam = new URLSearchParams(window.location.search).get("axis");
  if (axisParam && ["speed", "memory", "reliability", "cycle", "loc"].includes(axisParam)) {
    selectAxis(axisParam);
  }

  const io = new IntersectionObserver((entries) => {
    entries.forEach((e) => { if (e.isIntersecting) e.target.classList.add("in"); });
  }, { threshold: 0.12 });
  document.querySelectorAll("[data-reveal]").forEach((el) => io.observe(el));
})();
""".strip()


def render_benchmarks(report: dict[str, Any]) -> str:
    composite = report.get("composite_scores") or {}
    winner = report.get("overall_winner") or "n/a"
    cat_w = report.get("category_winners") or {}
    mach = report.get("machine") or {}
    weights = report.get("score_weights") or {}
    algos = report.get("algorithms") or []
    loc = report.get("loc") or {}

    win_score = (composite.get(winner) or {}).get("score")
    win_disp = display_name(winner)
    score_line = f"{win_score:.1f}" if isinstance(win_score, (int, float)) else "n/a"

    ranked = sorted(composite.keys(), key=lambda t: -composite[t]["score"]) if composite else []

    list_rows = []
    pickers = []
    for t in ranked:
        c = composite[t]
        dn = display_name(t)
        list_rows.append(
            f'<button type="button" class="lang-line" data-lang="{_esc(t)}" data-reveal>'
            f'<span class="lang-name">{_esc(dn)}</span>'
            f'<span class="lang-score">{c["score"]:.1f}</span>'
            f"</button>"
        )
        pickers.append(
            f'<button type="button" class="pick" data-lang="{_esc(t)}">{_esc(dn)}</button>'
        )
    static_score_rows = "".join(
        f"<tr><th scope=\"row\">{_esc(display_name(t))}</th>"
        f"<td>{float(composite[t]['score']):.2f}</td></tr>"
        for t in ranked
    )
    static_scores = (
        "<noscript><h3>Composite scores</h3>"
        "<div class=\"table-wrap\"><table class=\"data\">"
        "<thead><tr><th>Language</th><th>Score</th></tr></thead>"
        f"<tbody>{static_score_rows}</tbody></table></div></noscript>"
    )

    weight_segs = []
    for key, label, pct in WEIGHT_LABELS:
        w = float(weights.get(key, 0))
        flex = max(12, int(round(w * 100)))
        weight_segs.append(
            f'<div class="wseg" style="flex-grow:{flex}" tabindex="0">'
            f"<strong>{_esc(pct)}</strong>"
            f"<span>{_esc(label)}</span>"
            f"</div>"
        )

    cat_tiles_html = build_cat_tiles_html(cat_w)

    chart_btns = (
        '<button type="button" class="is-active" data-axis="score">Composite</button>'
        + "".join(
            f'<button type="button" data-axis="{_esc(k)}">{_esc(lab)}</button>'
            for k, lab, _ in WEIGHT_LABELS
        )
    )

    targets, timing_rows = build_timings_matrix(algos)
    head_cells = "".join(
        f'<th class="num" data-col="{_esc(t)}">{_esc(display_name(t))}</th>' for t in targets
    )
    matrix_body = []
    timing_cards = []
    for row in timing_rows:
        cells = ['<th scope="row" data-col="algo">' + _esc(row["name"]) + "</th>"]
        means = [row["means"].get(t) for t in targets]
        best = min((m for m in means if isinstance(m, (int, float))), default=None)
        card_rows = []
        for t in targets:
            m = row["means"].get(t)
            cls = "num"
            is_best = best is not None and isinstance(m, (int, float)) and abs(m - best) < 1e-12
            if is_best:
                cls += " is-best-cell"
            cells.append(
                f'<td class="{cls}" data-col="{_esc(t)}">'
                f'{_esc(_fmt_s(m) if m is not None else "n/a")}</td>'
            )
            card_rows.append(
                f'<div data-lang-row="{_esc(t)}"><dt>{_esc(display_name(t))}</dt>'
                f'<dd class="{"is-best" if is_best else ""}">'
                f'{_esc(_fmt_s(m) if m is not None else "n/a")}</dd></div>'
            )
        matrix_body.append(f"<tr>{''.join(cells)}</tr>")
        # find matching algo for winner chip
        win_name = None
        for algo in algos:
            if algo.get("name") == row["name"]:
                win_name = algo_overall_winner(algo)
                break
        chip = (
            f'<span class="winner-chip">{_esc(display_name(win_name))}</span>'
            if win_name
            else ""
        )
        timing_cards.append(
            f'<article class="timing-card" data-reveal>'
            f'<div class="algo-head"><h4>{_esc(row["name"])}</h4>{chip}</div>'
            f'<dl>{"".join(card_rows)}</dl>'
            f"</article>"
        )

    meth = report.get("methodology") or report.get("protocol") or {}
    proto_ver = meth.get("protocol_version") or (report.get("protocol") or {}).get("version") or "?"
    warm_n = meth.get("warmups", "?")
    timed_n = meth.get("timed_runs", "?")
    estimator = meth.get("estimator") or meth.get("wall_primary") or "median"
    meth_blurb = (
        f"Protocol <code>{_esc(str(proto_ver))}</code>: {_esc(str(warm_n))} warm-up(s) "
        f"discarded, {_esc(str(timed_n))} timed runs interleaved round-robin, "
        f"primary wall = <strong>{_esc(str(estimator))}</strong> with stdev/min/max and CI. "
        f"Overlapping CIs are within noise — never a hard order."
    )

    algo_blocks = []
    for algo in algos:
        name = algo.get("name", "")
        win = algo_overall_winner(algo)
        win_chip = (
            f'<span class="winner-chip" title="Fastest primary wall for this algorithm">'
            f'{_esc(display_name(win))}</span>'
            if win
            else ""
        )
        bars = []
        means = [
            w
            for w in (_wall_reading(t.get("stats")) for t in algo.get("targets", []) if t.get("status") == "ok")
            if w is not None
        ]
        max_mean = max(means) if means else 1.0
        ranking = algo.get("ranking") or []
        noise_note = ""
        if ranking:
            soft = [r for r in ranking if r.get("within_noise_of")]
            if soft:
                noise_note = (
                    f" · within-noise pairs: "
                    + ", ".join(
                        f"{r.get('target')}↔{'/'.join(r.get('within_noise_of') or [])}"
                        for r in soft[:4]
                    )
                )
        for t in algo.get("targets", []):
            st = t.get("status")
            mean = _wall_reading(t.get("stats")) if st == "ok" else None
            if st == "ok" and mean is not None:
                w = 100.0 * (mean / max_mean) if max_mean else 0
                is_win = win and t.get("target") == win
                sd = (t.get("stats") or {}).get("stddev")
                sd_s = f" ±{sd:.4f}" if isinstance(sd, (int, float)) else ""
                bars.append(
                    f"<div class='abar{' is-algo-winner' if is_win else ''}'>"
                    f"<span>{_esc(display_name(t.get('target')))}</span>"
                    f"<div class='atrack'><div class='afill' style='width:{w:.1f}%'></div></div>"
                    f"<em>{mean:.4f}s{sd_s}</em></div>"
                )
        algo_blocks.append(
            f"<article class='algo' id='algo-{_esc(name)}' data-reveal>"
            f"<div class='algo-head'><h3>{_esc(name)}</h3>{win_chip}</div>"
            f"<p class='note'>gate={_esc(algo.get('gate'))} · "
            f"ref={_esc(algo.get('reference_checksum'))}{_esc(noise_note)}</p>"
            f"<div class='achart' data-ui>{''.join(bars)}</div>"
            f"</article>"
        )

    faq = faq_items()
    faq_btns = "".join(
        f'<button type="button" class="faq-q{" is-active" if i == 0 else ""}" data-faq="{i}" data-reveal>'
        f"{_esc(q)}</button>"
        for i, (q, _) in enumerate(faq)
    )
    faq_bands = "".join(
        f'<div class="faq-band" data-reveal><h3>{_esc(q)}</h3><p>{_esc(a)}</p></div>'
        for q, a in faq
    )

    tools_rows = "".join(
        f"<tr><td>{_esc(display_name(t.get('name')))}</td>"
        f"<td>{'yes' if t.get('available') else 'no'}</td>"
        f"<td>{_esc(t.get('version') or '')}</td></tr>"
        for t in report.get("toolchains") or []
    )
    loc_rows = "".join(
        f"<tr><td>{_esc(display_name(lang))}</td><td class='num'>{v.get('code', 0)}</td></tr>"
        for lang, v in (loc.get("by_language") or {}).items()
    )

    display_map = {t: display_name(t) for t in ranked}
    display_map["all"] = "All"
    static_score_rows = "".join(
        f"<tr><td>{_esc(display_name(t))}</td>"
        f"<td class=\"num\">{composite[t]['score']:.2f}</td>"
        f"<td class=\"num\">{composite[t]['speed']:.2f}</td>"
        f"<td class=\"num\">{composite[t]['memory']:.2f}</td>"
        f"<td class=\"num\">{composite[t]['reliability']:.2f}</td>"
        f"<td class=\"num\">{composite[t]['cycle']:.2f}</td>"
        f"<td class=\"num\">{composite[t]['loc']:.2f}</td></tr>"
        for t in ranked
    )
    payload = {
        "winner": winner,
        "display": display_map,
        "weights": weights,
        "category": public_category_winners(cat_w),
        "composite": {
            t: {
                "score": composite[t]["score"],
                "speed": composite[t]["speed"],
                "memory": composite[t]["memory"],
                "reliability": composite[t]["reliability"],
                "cycle": composite[t]["cycle"],
                "loc": composite[t]["loc"],
                "contrib": composite[t].get("contrib") or {},
            }
            for t in ranked
        },
        "faq": [a for _, a in faq],
    }
    payload_json = json.dumps(payload, separators=(",", ":"))

    return f"""{head("Super · Benchmarks", page="benchmarks.html")}
{nav_html("benchmarks.html")}

<section class="hero-board" aria-label="Overall and category winners">
  <div class="tile tile-winner">
    <h1>
      <span class="rise-wrap"><span class="rise">{_esc(win_disp)}</span></span>
    </h1>
    <p class="win-score">{_esc(score_line)}</p>
    <p class="hero-support">Overall composite leader, measured on a single machine. Select a category winner to re-rank the chart by that axis.</p>
    <div class="hero-actions">
      <a class="cta" href="#live-chart">Open the chart</a>
      <a class="cta-ghost" href="#timings">Wall time</a>
    </div>
  </div>
  <div class="tile tile-cats">
    <h2>Category winners</h2>
    <div class="cat-grid" data-ui>{cat_tiles_html}</div>
  </div>
  <div class="tile tile-axis">
    <p class="axis-live" id="axis-live">{_esc(win_disp)} leads the composite</p>
    <div class="axis-meter" data-ui aria-hidden="true"><i id="axis-meter-fill"></i></div>
    <p class="live-hint" style="margin-top:1rem">Weights are listed below. Speed 40 · Memory 25 · Reliability 15 · Cycle 10 · LoC 10.</p>
  </div>
</section>

<noscript>
<section class="section" aria-label="Static benchmark score table">
  <h2>Composite scores</h2>
  <div class="table-wrap"><table class="data">
    <thead><tr><th>Language</th><th>Composite</th><th>Speed</th><th>Memory</th><th>Reliability</th><th>Cycle</th><th>LoC</th></tr></thead>
    <tbody>{static_score_rows}</tbody>
  </table></div>
</section>
</noscript>

<section class="section band-ink" id="live-chart">
  <h2>Composite chart</h2>
  <p class="note chart-note">Composite scores from the Stage-1 suite. Use the axis buttons to re-rank the chart, and select a bar to see that language's score breakdown.</p>
  <div class="chart-toolbar" role="toolbar" aria-label="Chart axis">{chart_btns}</div>
  <div class="chart-panel" data-reveal>
    <div id="rank-chart"></div>
  </div>
  {static_scores}
</section>

<section class="section" id="score">
  <div class="stats-wrap">
    <div>
      <h2>The composite</h2>
      <p class="total" id="live-total">{_esc(f"{win_score:.1f}" if isinstance(win_score, (int, float)) else "n/a")}</p>
      <p class="total-cap" id="live-cap">Weighted sum for <strong id="live-lang">{_esc(win_disp)}</strong>. Each axis is ratio-to-best, then multiplied by its weight.</p>
      <p class="live-hint">Select a language to see its score breakdown and radar.</p>
      <div class="picks" role="list">{''.join(pickers)}</div>
      <div class="radar-wrap" id="radar-chart" data-reveal></div>
    </div>
    <aside class="receipt" id="receipt" data-ui aria-live="polite">
      <h3>Score receipt</h3>
      <div id="receipt-body"></div>
      <div class="receipt-total"><span>Composite</span><span id="receipt-sum">n/a</span></div>
    </aside>
  </div>
</section>

<section class="section" id="methodology">
  <h2>Timing protocol</h2>
  <p class="note" style="max-width:60ch">{meth_blurb}</p>
  <p class="note" style="max-width:60ch;opacity:0.78">Every raw run, including warm-ups, is published alongside the aggregates so results can be re-checked and re-aggregated. All timings come from one machine; see the toolchain section below and REPRODUCE.md to rerun the suite on your own hardware.</p>
</section>

<section class="section band-field" id="timings">
  <h2>Wall time</h2>
  <p class="note" style="color:var(--field-ink);max-width:52ch;opacity:0.72">Primary wall ({_esc(str(estimator))}) in seconds with stdev where available. Best per algorithm is marked. Overlapping CIs are within noise.</p>
  <div class="matrix-filter" id="matrix-filter"></div>
  <div class="timings-layout">
    {''.join(algo_blocks) if algo_blocks else '<p>No algorithm results yet.</p>'}
    <div class="table-wrap" data-reveal>
      <table class="data" id="timings-table">
        <thead>
          <tr><th scope="col" data-col="algo">Algorithm</th>{head_cells}</tr>
        </thead>
        <tbody>
          {''.join(matrix_body) if matrix_body else '<tr><td colspan="99">No timings yet.</td></tr>'}
        </tbody>
      </table>
    </div>
    <div class="timing-cards" id="timing-cards">{''.join(timing_cards)}</div>
  </div>
</section>

<section class="section band-ink" id="race">
  <h2>Score race</h2>
  <div class="ui-stage" data-reveal>
    <canvas id="score-race" width="960" height="288" data-ui aria-label="Animated composite score race"></canvas>
    <p class="ui-caption">Animated view of the composite scores; the leading bar is highlighted.</p>
  </div>
</section>

<section class="section" id="weights">
  <h2>How the score is built</h2>
  <p class="note" style="max-width:48ch">Scoring weights for the composite. Speed still carries the most; the other axes keep a fast incomplete port from winning.</p>
  <div class="weight-bar" data-reveal>{''.join(weight_segs)}</div>
</section>

<section class="lang-list band-surface" id="languages">
  {''.join(list_rows) if list_rows else '<p>No composite scores yet.</p>'}
</section>

<section class="section" id="method">
  <div class="faq-grid">
    <div>
      <h2>Why these weights</h2>
      <div id="faq-list">{faq_btns}</div>
    </div>
    <p class="faq-answer" id="faq-answer" data-reveal>{_esc(faq[0][1])}</p>
  </div>
</section>

<section class="faq-bands" id="faq-full" aria-label="Methodology detail">
  <div class="faq-band" style="background:var(--ink);color:var(--accent-ink);min-height:30svh;align-items:end">
    <h3 style="color:var(--accent-ink);font-size:clamp(2rem,7vw,4.5rem);max-width:10ch">Why these weights</h3>
    <p style="color:#ffb07a">Each axis guards against a different failure mode.</p>
  </div>
  {faq_bands}
</section>

<section class="showcase" id="showcase">
  <h2>Forty percent</h2>
  <p>Speed still leads the composite. The other axes keep a fast incomplete port from winning.</p>
  <p class="blown" data-reveal>40%</p>
</section>

<section class="cta-band" id="book-cta">
  <div class="cta-main">
    <h2>Read the book</h2>
    <p style="color:#ffb07a;max-width:34ch">Specification, algorithms, ECS and APIs. Read it online or download the PDF.</p>
    <div class="hero-actions" style="margin-top:1.5rem">
      <a class="cta" href="book.html" style="background:var(--accent-ink);color:var(--ink)">Open book</a>
    </div>
  </div>
  <aside class="cta-side book-panel">
    <h3>Super Book</h3>
    <p>Page-turn reader with the full text. PDF available for offline reading.</p>
    <a class="cta" href="book.html">Open viewer</a>
    <p style="margin-top:1rem"><a href="downloads/SUPER_BOOK.pdf" download>Download SUPER_BOOK.pdf</a></p>
  </aside>
</section>

<section class="section band-field" id="toolchain">
  <h2>Toolchain and LoC</h2>
  <p class="note" style="color:var(--field-ink);max-width:52ch;opacity:0.78">Tool versions used for this suite and the primary source size per language. Both describe the single machine behind the scoreboard.</p>
  <div class="toolchain-grid">
    <div class="toolchain-panel" data-reveal>
      <h3>Toolchain versions</h3>
      <div class="table-wrap">
        <table class="data data-stack">
          <thead><tr><th>Tool</th><th>Avail</th><th>Version</th></tr></thead>
          <tbody>{tools_rows or '<tr><td colspan="3">n/a</td></tr>'}</tbody>
        </table>
      </div>
    </div>
    <div class="toolchain-panel" data-reveal>
      <h3>Code lines</h3>
      <div class="table-wrap">
        <table class="data data-stack">
          <thead><tr><th>Language</th><th class="num">Lines</th></tr></thead>
          <tbody>{loc_rows or '<tr><td colspan="2">n/a</td></tr>'}</tbody>
        </table>
      </div>
    </div>
  </div>
</section>

{footer_html(mach, report.get("generated_at"))}

<script type="application/json" id="report-data">{payload_json}</script>
<script>
{benchmarks_js()}
</script>
</body>
</html>
"""


SAMPLE_SR = """+TaxRate = 0.14

+User = {
  id: i64
  name: str
  balance: f64 = 0.0

  discount(@self, percent: f64) : f64 {
    cut = self.balance * (percent / 100.0)
    self.balance -= cut
    self.balance
  }
}

+add_tax(amount: f64) => amount * (1.0 + TaxRate)

final = @u.discount(10.0).add_tax().round(2)
"""


def _winner_board_html(
    report: dict[str, Any], *, chart_href: str | None = None
) -> tuple[str, str, str]:
    """Shared overall + category winner tiles for index and winners.html."""
    composite = report.get("composite_scores") or {}
    winner = report.get("overall_winner") or "n/a"
    cat_w = report.get("category_winners") or {}
    win_score = (composite.get(winner) or {}).get("score")
    win_disp = display_name(winner)
    score_line = f"{win_score:.1f}" if isinstance(win_score, (int, float)) else "n/a"
    return win_disp, score_line, build_cat_tiles_html(cat_w, chart_href=chart_href)


def render_winners(report: dict[str, Any]) -> str:
    """Focused winners board — overall + categories, deep-links into the chart."""
    win_disp, score_line, cat_html = _winner_board_html(report, chart_href="../charts/")
    mach = report.get("machine") or {}
    return f"""{head("Super · Winners", page="winners.html")}
{nav_html("winners.html")}
<section class="hero-board" aria-label="Overall and category winners">
  <div class="tile tile-winner">
    <h1>
      <span class="rise-wrap"><span class="rise">{_esc(win_disp)}</span></span>
    </h1>
    <p class="win-score">{_esc(score_line)}</p>
    <p class="hero-support">Overall composite leader, measured on a single machine. Select a category winner to open it in the interactive chart.</p>
    <div class="hero-actions">
      <a class="cta" href="charts.html">Open charts</a>
      <a class="cta-ghost" href="benchmarks.html#timings">Wall time</a>
    </div>
  </div>
  <div class="tile tile-cats">
    <h2>Category winners</h2>
    <div class="cat-grid" data-ui>{cat_html}</div>
  </div>
  <div class="tile tile-axis">
    <p class="axis-live" id="axis-live">{_esc(win_disp)} leads the composite</p>
    <p class="live-hint" style="margin-top:1rem">Weights: Speed 40 · Memory 25 · Reliability 15 · Cycle 10 · LoC 10.</p>
  </div>
</section>
<section class="cta-band">
  <div class="cta-main" data-reveal>
    <h2>See the receipt</h2>
    <p style="color:#ffb07a;max-width:34ch">Full scoreboard with per-language composites and wall times.</p>
    <a class="cta" href="benchmarks.html" style="background:var(--accent-ink);color:var(--ink);margin-top:1.25rem">Open benchmarks</a>
  </div>
</section>
{footer_html(mach, report.get("generated_at"))}
<script>{site_js_shared()}</script>
</body>
</html>
"""


def render_charts(report: dict[str, Any]) -> str:
    """Composite chart focus page; loads the same results.json payload as index."""
    composite = report.get("composite_scores") or {}
    winner = report.get("overall_winner") or "n/a"
    cat_w = report.get("category_winners") or {}
    weights = report.get("score_weights") or {}
    mach = report.get("machine") or {}
    ranked = sorted(composite.keys(), key=lambda t: -composite[t]["score"]) if composite else []
    pickers = "".join(
        f'<button type="button" class="pick" data-lang="{_esc(t)}">{_esc(display_name(t))}</button>'
        for t in ranked
    )
    chart_btns = (
        '<button type="button" class="is-active" data-axis="score">Composite</button>'
        + "".join(
            f'<button type="button" data-axis="{_esc(k)}">{_esc(lab)}</button>'
            for k, lab, _ in WEIGHT_LABELS
        )
    )
    display_map = {t: display_name(t) for t in ranked}
    display_map["all"] = "All"
    payload = {
        "winner": winner,
        "display": display_map,
        "weights": weights,
        "category": public_category_winners(cat_w),
        "composite": {
            t: {
                "score": composite[t]["score"],
                "speed": composite[t]["speed"],
                "memory": composite[t]["memory"],
                "reliability": composite[t]["reliability"],
                "cycle": composite[t]["cycle"],
                "loc": composite[t]["loc"],
                "contrib": composite[t].get("contrib") or {},
            }
            for t in ranked
        },
    }
    payload_json = json.dumps(payload, ensure_ascii=False)
    return f"""{head("Super · Charts", page="charts.html")}
{nav_html("charts.html")}
<section class="section band-ink" id="live-chart">
  <h2>Composite chart</h2>
  <p class="note chart-note">Composite scores from the Stage-1 suite. Use the axis buttons to re-rank the chart, and select a bar to see that language's score breakdown.</p>
  <div class="chart-toolbar" role="toolbar" aria-label="Chart axis">{chart_btns}</div>
  <div class="chart-panel" data-reveal>
    <div id="rank-chart"></div>
  </div>
</section>
<section class="section" id="score">
  <div class="stats-wrap">
    <div>
      <h2>The composite</h2>
      <p class="total" id="live-total">—</p>
      <p class="total-cap" id="live-cap">Select a language to see its score breakdown and radar.</p>
      <div class="picks" role="list">{pickers}</div>
      <div class="radar-wrap" id="radar-chart" data-reveal></div>
    </div>
    <aside class="receipt" id="receipt" data-ui aria-live="polite">
      <h3>Score receipt</h3>
      <div id="receipt-body"></div>
      <div class="receipt-total"><span>Composite</span><span id="receipt-sum">n/a</span></div>
    </aside>
  </div>
</section>
<section class="section">
  <div class="hero-actions">
    <a class="cta" href="winners.html">Winners board</a>
    <a class="cta-ghost" href="benchmarks.html">Full benchmarks</a>
  </div>
</section>
{footer_html(mach, report.get("generated_at"))}
<script type="application/json" id="report-data">{payload_json}</script>
<script>{site_js_shared()}</script>
<script>
{benchmarks_js()}
</script>
</body>
</html>
"""


_INSTALL_ONELINER = (
    "sh -c 'f=$(mktemp) && trap \"rm -f -- \\\"$f\\\"\" 0 && "
    "curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh "
    '-o "$f" && sh "$f" "$@"\' sh'
)


def landing_excerpt_html() -> str:
    return f"""
<section class="landing-excerpt" id="what-is-super">
  <h1>Super for APIs and ECS</h1>
  <p class="landing-lead">Super (<code>.sr</code>) is a stack-oriented language with low-boilerplate expression syntax, scoped data stacks, heavy comptime, C23 AOT, a bytecode VM, and a numeric-loop JIT — plus optional law/proof files beside ordinary tests. One tree feeds SuperAot and SuperJit for backends and data-oriented engines.</p>
  <ul class="landing-points" data-reveal>
    <li data-reveal>Expression syntax without <code>fn</code> / <code>let</code> ceremony</li>
    <li data-reveal>Frame-local allocation via scoped data stacks (Memory axis leader)</li>
    <li data-reveal><code>super verify</code> for law/proof sidecars (experimental checker)</li>
  </ul>
  <div class="hero-actions" data-reveal>
    <a class="cta" href="quickstart.html">Quickstart</a>
    <a class="cta" href="install.html">Install Super</a>
    <a class="cta-ghost" href="http-server.html">Build an API</a>
    <a class="cta-ghost" href="docs-hub.html">Docs</a>
    <a class="cta-ghost" href="benchmarks.html">Scoreboard</a>
    <a class="cta-ghost" href="spec.html">Read the spec</a>
  </div>
  <div class="install-copy" data-ui data-reveal>
    <p class="install-copy-label">Install in one line</p>
    <pre><code id="install-snippet">{_esc(_INSTALL_ONELINER)}</code></pre>
    <button type="button" class="cta" data-copy-target="install-snippet">Copy command</button>
  </div>
</section>
""".strip()


def _render_language_body(*, page: str, nav_active: str, title: str) -> str:
    return f"""{head(title, page=page)}
{nav_html(nav_active)}
{landing_excerpt_html()}
<section class="section">
  <h2>Syntax without ceremony</h2>
  <div class="code-stage" data-ui data-reveal>
    <pre class="sr-block"><code class="language-sr">{_esc(SAMPLE_SR)}</code></pre>
  </div>
</section>
<section class="section band-surface">
  <h2>Two runtimes</h2>
  <div class="runtime-board" data-ui data-reveal>
    <div data-reveal><strong>SuperAot</strong><em>Native ahead-of-time binary from algo.sr</em></div>
    <div data-reveal><strong>SuperJit</strong><em>Sub-millisecond hot reload path</em></div>
    <div data-reveal><strong>UFCS</strong><em>@ mutation, implicit it, value errors</em></div>
    <div data-reveal><strong>Pod.toml</strong><em>One manifest for deps, targets, profiles</em></div>
  </div>
</section>
<section class="section band-ink">
  <h2>Scoreboard</h2>
  <p class="note chart-note">Stage-1 composite scores are on the benchmarks page. The composite weights are:</p>
  <div class="weight-poster" data-ui data-reveal>
    <p class="weight-line" data-reveal><strong>40</strong> Speed</p>
    <p class="weight-line" data-reveal><strong>25</strong> Memory</p>
    <p class="weight-line" data-reveal><strong>15</strong> Reliability</p>
    <p class="weight-line" data-reveal><strong>10</strong> Cycle</p>
    <p class="weight-line" data-reveal><strong>10</strong> LoC</p>
  </div>
</section>
<section class="section">
  <h2>Pipeline shape</h2>
  <div class="pipeline-board" data-ui data-reveal>
    <ol data-stagger>
      <li data-reveal>Lex · arena tokens</li>
      <li data-reveal>Parse · NodeId AST</li>
      <li data-reveal>Sema · single-pass types</li>
      <li data-reveal>VM / JIT · hot path</li>
    </ol>
  </div>
</section>
<section class="cta-band">
  <div class="cta-main" data-reveal>
    <h2>Prove it</h2>
    <p style="color:#ffb07a;max-width:42ch">Stage-1 equal-algorithm suite with checksum gates. Scores are frozen from one run on a single machine (<a href="https://github.com/theHamdiz/super-lang/blob/main/docs/REPORT.md">full report</a>). Zig wins Speed; overlapping confidence intervals on nbody, astar, and json are labelled within noise.</p>
    <a class="cta" href="benchmarks.html" style="background:var(--accent-ink);color:var(--ink);margin-top:1.25rem">Open scoreboard</a>
    <p style="margin-top:0.75rem"><a href="https://github.com/theHamdiz/super-lang/blob/main/benchmarks/REPRODUCE.md">REPRODUCE.md</a> · <a href="benchmarks.html#toolchain">Toolchain and LoC notes</a></p>
  </div>
  <aside class="cta-side book-panel" data-reveal>
    <h3>Read the book</h3>
    <p><a href="spec.html">Specification</a> (normative) · <a href="book.html">Book</a> (narrative) · <a href="downloads/SUPER_BOOK.pdf">PDF</a></p>
    <a class="cta" href="spec.html">Open spec</a>
  </aside>
</section>
{footer_html()}
<script>{site_js_shared()}</script>
</body>
</html>
"""


def render_home() -> str:
    return _render_language_body(page="index.html", nav_active="index.html", title="Super · Home")


def render_language() -> str:
    return """<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta http-equiv="refresh" content="0; url=../">
<link rel="canonical" href="https://thehamdiz.github.io/super-lang/">
<title>Super · Home</title></head>
<body><p><a href="../">Continue to the Super home page</a>.</p></body></html>"""


def render_install() -> str:
    one_liner = (
        "sh -c 'f=$(mktemp) && trap \"rm -f -- \\\"$f\\\"\" 0 && "
        "curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh "
        '-o "$f" && sh "$f" "$@"\' sh'
    )
    return f"""{head("Super · Install", page="install.html")}
{nav_html("install.html")}
<section class="page-hero">
  <h1>Install Super</h1>
  <p>One command installs the 0.1.0 toolchain into <code>~/.super</code> — outside <code>~/.cargo</code> — with CLI, stdlib, LSP, and MCP shims.</p>
</section>
<section class="section">
  <h2>One-liner</h2>
  <p class="prose">From any shell with <code>curl</code> (or FreeBSD <code>fetch</code>):</p>
  <div class="code-stage" data-ui data-reveal>
    <pre><code>{_esc(one_liner)}</code></pre>
  </div>
  <p class="prose" style="margin-top:1.25rem;color:var(--muted)">Prefers a GitHub release bundle for your OS/CPU; otherwise clones and builds with a stable Rust toolchain. Open a new shell after PATH is updated.</p>
</section>
<section class="section band-field">
  <h2>Where files land</h2>
  <div class="code-stage" data-ui data-reveal>
    <pre><code>~/.super/
  bin/super
  bin/super-lsp      # → super lsp
  bin/super-mcp      # → super mcp
  lib/stdlib/        # use std.*
  registry/          # super publish / version deps
  cache/             # git Pods</code></pre>
  </div>
  <p class="prose" style="margin-top:1.25rem;color:var(--muted)">Override with <code>SUPER_HOME</code>, or set <code>SUPER_USE_XDG=1</code> for <code>$XDG_DATA_HOME/super</code>. Runtime still honors <code>SUPER_STD</code> / <code>SUPER_STDLIB</code>.</p>
</section>
<section class="section">
  <h2>From a checkout</h2>
  <div class="code-stage" data-ui data-reveal>
    <pre><code>git clone https://github.com/theHamdiz/super-lang.git
cd super-lang
./scripts/install.sh
# or: make install

super --help
super check examples/01_space_shooter/src/main.sr
super test  examples/01_space_shooter/
super run   examples/01_space_shooter/src/main.sr</code></pre>
  </div>
</section>
<section class="section band-surface">
  <h2>After install</h2>
  <div class="intro-grid">
    <div data-reveal><strong>Benchmarks</strong><em>View the scoreboard</em></div>
    <div data-reveal><strong>Book</strong><em>Read the language book online</em></div>
    <div data-reveal><strong>Examples</strong><em>Game, API, log pipeline</em></div>
    <div data-reveal><strong>Agents</strong><em><code>super lsp</code> · <code>super mcp</code></em></div>
  </div>
  <div class="hero-actions" style="margin-top:2rem">
    <a class="cta" href="benchmarks.html">Open benchmarks</a>
    <a class="cta-ghost" href="downloads/SUPER_BOOK.pdf" download>Download book</a>
  </div>
</section>
{footer_html()}
<script>{site_js_shared()}</script>
</body>
</html>
"""


def _discover_example_sources() -> list[dict[str, str]]:
    """All runnable example project sources.

    Frozen benchmark programs remain on the benchmark/reproduction pages and
    are deliberately not republished as current API examples.
    """
    items: list[dict[str, str]] = []
    ex_root = REPO / "examples"
    if ex_root.is_dir():
        for proj in sorted(ex_root.iterdir()):
            if not proj.is_dir() or proj.name.startswith("."):
                continue
            files = sorted(proj.rglob("*.sr"))
            if not files:
                continue
            # Prefer main.sr then others; keep multi-file projects together
            for f in files:
                rel = f.relative_to(REPO).as_posix()
                items.append(
                    {
                        "kind": "project",
                        "id": f"ex-{proj.name}-{f.stem}",
                        "title": proj.name.replace("_", " "),
                        "path": rel,
                        "file": f.name,
                        "src": f.read_text(encoding="utf-8", errors="replace"),
                    }
                )
    return items


def render_examples() -> str:
    items = _discover_example_sources()
    # Deduplicate display: group by project/algorithm folder
    nav_bits: list[str] = []
    seen_titles: set[str] = set()
    for it in items:
        key = it["title"]
        if key in seen_titles:
            continue
        seen_titles.add(key)
        nav_bits.append(
            f'<a href="#{_esc(it["id"])}">{_esc(it["title"])}</a>'
        )

    sections: list[str] = []
    for it in items:
        src = it["src"]
        truncated = False
        lines = src.splitlines()
        if len(lines) > 420:
            src = "\n".join(lines[:420]) + "\n// … truncated for the page; see " + it["path"]
            truncated = True
        kind = "Algorithm" if it["kind"] == "algorithm" else "Project"
        sections.append(
            f'<article class="example-block" id="{_esc(it["id"])}" data-reveal>'
            f'<div class="example-meta"><span class="example-kind">{_esc(kind)}</span>'
            f'<h3>{_esc(it["title"])}</h3>'
            f'<p class="note">{_esc(it["path"])}'
            f'{" · truncated" if truncated else ""}</p></div>'
            f'<pre class="sr-block"><code class="language-sr">{_esc(src)}</code></pre>'
            f"</article>"
        )

    return f"""{head("Super · Examples", page="examples.html")}
{nav_html("examples.html")}
<section class="page-hero">
  <h1>Projects and algorithms</h1>
  <p>Every example project and every benchmark algorithm, with Super source highlighted.</p>
</section>
<section class="section band-ink">
  <h2>Jump</h2>
  <div class="example-nav" data-reveal>{''.join(nav_bits) if nav_bits else "<p>No .sr sources found.</p>"}</div>
</section>
<section class="section examples-stage">
  {''.join(sections) if sections else "<p>No examples found.</p>"}
</section>
<section class="cta-band">
  <div class="cta-main">
    <h2>Clone and run</h2>
    <p style="color:#ffb07a;max-width:34ch">Install the CLI, then open each path under examples/ or benchmarks/.</p>
  </div>
  <aside class="cta-side book-panel">
    <h3>Install</h3>
    <p>Build super-cli and point SUPER_STD at stdlib.</p>
    <a class="cta" href="install.html">Install steps</a>
  </aside>
</section>
{footer_html()}
<script>{site_js_shared()}</script>
</body>
</html>
"""


def render_book() -> str:
    return f"""{head("Super · Book", page="book.html")}
{nav_html("book.html")}
<section class="books-shelf" id="books-shelf">
  <h1>Super Book</h1>
  <p style="max-width:34ch;margin:0;color:var(--muted)">Open the cover. Click either page, use the buttons, or press the arrow keys.</p>
  <div class="shelf-row">
    <button type="button" class="cover-card" data-open-book data-ui aria-label="Open Super Book">
      <strong>The Super Programming Language</strong>
      <span>Ahmad Hamdi · 0.1.0</span>
    </button>
  </div>
  <div class="hero-actions">
    <button type="button" class="cta" id="open-book">Open book</button>
    <a class="cta-ghost" href="downloads/SUPER_BOOK.pdf" download>Download PDF</a>
  </div>
</section>

<div class="reader" id="reader" role="dialog" aria-modal="true" aria-label="Super Book reader">
  <div class="reader-bar">
    <button type="button" id="close-reader">Close</button>
    <span id="page-label">1 / 1</span>
    <div>
      <button type="button" id="prev-page">Prev</button>
      <button type="button" id="next-page">Next</button>
      <a href="downloads/SUPER_BOOK.pdf" download>PDF</a>
    </div>
  </div>
  <div class="reader-progress"><i id="reader-progress"></i></div>
  <div class="reader-stage">
    <div class="book-spread" id="book-spread" data-ui>
      <div class="sheet under left" id="under-left">
        <div class="page-inner"></div>
        <span class="page-num"></span>
      </div>
      <div class="sheet under right" id="under-right">
        <div class="page-inner"></div>
        <span class="page-num"></span>
      </div>
      <div class="sheet hard left page" id="page-left">
        <div class="face front">
          <div class="page-inner"></div>
          <span class="page-num"></span>
        </div>
        <div class="face back">
          <div class="page-inner"></div>
          <span class="page-num"></span>
        </div>
        <div class="page-edge" aria-hidden="true"></div>
        <div class="page-curl" aria-hidden="true"></div>
      </div>
      <div class="sheet hard right page" id="page-right">
        <div class="face front">
          <div class="page-inner"></div>
          <span class="page-num"></span>
        </div>
        <div class="face back">
          <div class="page-inner"></div>
          <span class="page-num"></span>
        </div>
        <div class="page-edge" aria-hidden="true"></div>
        <div class="page-curl" aria-hidden="true"></div>
      </div>
    </div>
  </div>
  <p class="reader-hint">Click left / right · arrows · space · esc</p>
</div>

{footer_html()}
<script src="book-flip.js"></script>
</body>
</html>
"""


def _spec_excerpt() -> str:
    spec = REPO / "SPECIFICATION.md"
    if not spec.is_file():
        return "<p>SPECIFICATION.md not found in the repository.</p>"
    lines = spec.read_text(encoding="utf-8", errors="replace").splitlines()
    excerpt = lines[:260]
    if sum(1 for line in excerpt if line.lstrip().startswith("```")) % 2:
        excerpt.append("```")
    sys.path.insert(0, str(REPO / "scripts"))
    from md_render import render as render_markdown

    parts: list[str] = [render_markdown("\n".join(excerpt), heading_offset=1)]
    stdlib = sorted((REPO / "stdlib").rglob("*.sr"))
    sr_files = len(stdlib)
    impl_modules = sum(
        1
        for p in stdlib
        if p.name != "mod.sr" and not p.name.endswith((".law.sr", ".proof.sr"))
    )
    # Stage-1 surface that lives past the opening excerpt (stdlib / dispatch / install).
    parts.append("<h3>Stage-1 language and stdlib (0.1.0)</h3>")
    parts.append(
        "<ul>"
        "<li><code>std.html.template</code> — Html/Text templates with comptime T001–T004.</li>"
        "<li><code>std.sec.secret</code> vs <code>std.sec.vault</code> — process-local Secret/FileKeyring vs Vault-shaped named store.</li>"
        "<li>Fixed-width ints <code>i8</code>…<code>u64</code>; channel multiplexing via <code>select { … }</code>.</li>"
        "<li><code>std.datetime</code> (civil/TZ/ISO) distinct from <code>std.sys.time</code>.</li>"
        "<li>Multiple dispatch: <code>FuncFamily</code>, <code>#symmetric</code>, multi-subject <code>when</code>.</li>"
        "<li><code>std.math</code> scientific suite; <code>std.data</code> + <code>std.fmt.xlsx</code>.</li>"
        "<li><code>super pod pack</code> / <code>verify</code>; toolchain under <code>~/.super</code> (<code>SUPER_HOME</code>).</li>"
        f"<li>Stdlib ships <strong>{sr_files}</strong> <code>.sr</code> files / <strong>{impl_modules}</strong> implementation modules.</li>"
        "</ul>"
    )
    parts.append(
        '<p class="note">Opening excerpt plus Stage-1 highlights from SPECIFICATION.md. '
        '<a href="downloads/SUPER_BOOK.pdf">Download the book PDF</a> or open the '
        '<a href="book.html">online book reader</a> for the full text.</p>'
    )
    return "\n".join(parts)


def render_spec() -> str:
    return f"""{head("Super · Spec", page="spec.html")}
{nav_html("spec.html")}
<section class="page-hero">
  <h1>Language surface</h1>
  <p>Online notes from SPECIFICATION.md. Download the book for the complete text.</p>
  <div class="hero-actions">
    <a class="cta" href="downloads/SUPER_BOOK.pdf" download>Download book</a>
    <a class="cta-ghost" href="book.html">Open book viewer</a>
  </div>
</section>
<section class="section band-ink">
  <div class="ui-stage" data-reveal style="min-height:8rem;display:flex;align-items:flex-end">
    <p style="margin:0;font-size:clamp(1.75rem,5vw,3rem);letter-spacing:-0.04em;line-height:0.95;max-width:18ch">Spec notes · full text in the online book reader</p>
  </div>
</section>
<article class="spec-body" data-reveal>
{_spec_excerpt()}
</article>
{footer_html()}
<script>{site_js_shared()}</script>
</body>
</html>
"""


def _copy_assets(dest: Path) -> None:
    ui_src = ASSETS_SRC / "ui"
    ui_dst = dest / "assets" / "ui"
    ui_dst.mkdir(parents=True, exist_ok=True)
    if ui_src.is_dir():
        for p in ui_src.iterdir():
            if p.is_file():
                # skip screenshot stand-ins; interactive components replaced them
                if p.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp"}:
                    continue
                shutil.copy2(p, ui_dst / p.name)
    # Self-hosted Cascadia + JetBrains Mono for book/site code
    fonts_src = SITE_ASSETS / "fonts"
    fonts_dst = dest / "assets" / "fonts"
    if fonts_src.is_dir():
        fonts_dst.mkdir(parents=True, exist_ok=True)
        for p in fonts_src.iterdir():
            if p.is_file() and p.suffix.lower() in {".woff2", ".ttf", ".otf", ".woff"}:
                shutil.copy2(p, fonts_dst / p.name)
    dl = dest / "downloads"
    dl.mkdir(parents=True, exist_ok=True)
    for src, name in [
        (REPO / "SUPER_BOOK.pdf", "SUPER_BOOK.pdf"),
        (REPO / "book" / "super-book.pdf", "super-book.pdf"),
        (ROOT / "report.pdf", "report.pdf"),
    ]:
        if src.is_file():
            shutil.copy2(src, dl / name)
    # keep existing PDF in www/downloads if repo copy missing
    existing = WWW / "downloads" / "SUPER_BOOK.pdf"
    if existing.is_file() and not (dl / "SUPER_BOOK.pdf").is_file():
        shutil.copy2(existing, dl / "SUPER_BOOK.pdf")


SITE_HTML_PAGES = (
    "site.css",
    "index.html",
    "benchmarks/index.html",
    "language/index.html",
    "install/index.html",
    "examples/index.html",
    "book/index.html",
    "spec/index.html",
    "report/index.html",
    "winners/index.html",
    "charts/index.html",
    "book-flip.js",
    "super-highlight.js",
    "book-pages.json",
    "results.json",
)


def sync_www(dest: Path | None = None) -> Path:
    """Copy generated docs/ site tree to a live serve directory (default /tmp/bm_www)."""
    target = Path(dest) if dest else Path("/tmp/bm_www")
    target.mkdir(parents=True, exist_ok=True)
    for name in SITE_HTML_PAGES:
        src = WWW / name
        if src.is_file():
            dst = target / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
    if (WWW / "assets").is_dir():
        if (target / "assets").exists():
            shutil.rmtree(target / "assets")
        shutil.copytree(WWW / "assets", target / "assets")
    if (WWW / "downloads").is_dir():
        if (target / "downloads").exists():
            shutil.rmtree(target / "downloads")
        shutil.copytree(WWW / "downloads", target / "downloads")
    for data in ("results.json", "REPORT.md"):
        src = ROOT / data
        if src.is_file():
            shutil.copy2(src, target / data)
    return target


def render_site(
    report: dict[str, Any],
    path: Path | None = None,
    *,
    sync: bool = True,
) -> Path:
    """Emit the multi-page site under docs/ (GitHub Pages) and mirror report.html."""
    WWW.mkdir(parents=True, exist_ok=True)
    # Preserve Jekyll scaffold dirs; only overwrite generated artifacts.
    for keep in ("_includes", "_layouts", "_sass", "_data", "assets"):
        (WWW / keep).mkdir(parents=True, exist_ok=True)
    (WWW / "site.css").write_text(site_css().rstrip() + "\n", encoding="utf-8")
    (WWW / "book-flip.js").write_text(book_flip_js().rstrip() + "\n", encoding="utf-8")
    (WWW / "super-highlight.js").write_text(
        super_highlight_js().rstrip() + "\n", encoding="utf-8"
    )
    # Mirror JS into assets/js for Jekyll asset pipeline consumers
    js_dir = WWW / "assets" / "js"
    js_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(WWW / "book-flip.js", js_dir / "book-flip.js")
    shutil.copy2(WWW / "super-highlight.js", js_dir / "super-highlight.js")
    results_src = ROOT / "results.json"
    if results_src.is_file() and not (WWW / "results.json").is_file():
        raw = json.loads(results_src.read_text(encoding="utf-8"))
        scrubbed = scrub_results_for_docs(raw)
        (WWW / "results.json").write_text(
            json.dumps(scrubbed, indent=2) + "\n", encoding="utf-8"
        )
    report_md = ROOT / "REPORT.md"
    # docs/REPORT.md is the frozen published measurement. Do not overwrite it
    # from test fixtures or a partial benchmark workspace during site builds.
    if report_md.is_file() and not (WWW / "REPORT.md").is_file():
        shutil.copy2(report_md, WWW / "REPORT.md")

    pdf = WWW / "downloads" / "SUPER_BOOK.pdf"
    if not pdf.is_file():
        alt = REPO / "SUPER_BOOK.pdf"
        if alt.is_file():
            (WWW / "downloads").mkdir(parents=True, exist_ok=True)
            shutil.copy2(alt, pdf)
    # Full book parity with the print PDF — no web abridgment
    book_data = extract_book_pages(pdf, max_pages=None)
    (WWW / "book-pages.json").write_text(
        json.dumps(book_data, ensure_ascii=False), encoding="utf-8"
    )
    import sys

    sys.path.insert(0, str(REPO / "scripts"))
    from site_urls import write_pretty_page

    bench = render_benchmarks(report)
    write_pretty_page(WWW, "benchmarks", bench)
    write_pretty_page(WWW, "report", bench)
    write_pretty_page(WWW, "", render_home())
    write_pretty_page(WWW, "winners", render_winners(report))
    write_pretty_page(WWW, "charts", render_charts(report))
    write_pretty_page(WWW, "language", render_language())
    write_pretty_page(WWW, "install", render_install())
    write_pretty_page(WWW, "examples", render_examples())
    write_pretty_page(WWW, "book", render_book())
    write_pretty_page(WWW, "spec", render_spec())
    _copy_assets(WWW)
    out = path or (ROOT / "report.html")
    out.write_text(bench, encoding="utf-8")
    if sync:
        sync_www()
        publish_mandara()
    return out


def publish_mandara(dest: Path | None = None) -> Path | None:
    """Publish the Super site into Mandara GitHub Pages tree as super/.

    Mandara serves static HTML (no Jekyll build required). Super keeps its
    orange/Bricolage art direction under /super/ with file-relative links so
    it works at https://thehamdiz.github.io/mandara/super/ and when opened
    from disk. Override with SUPER_MANDARA_ROOT.
    """
    target_root = Path(dest) if dest else MANDARA_ROOT
    if not target_root.is_dir():
        print(f"mandara root missing, skip publish: {target_root}")
        return None
    target = target_root / "super"
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True, exist_ok=True)
    # Mirror docs/ generated pages into Mandara/super (first-class site subtree)
    for name in SITE_HTML_PAGES:
        src = WWW / name
        if src.is_file():
            shutil.copy2(src, target / name)
    for folder in ("assets", "downloads"):
        src = WWW / folder
        if src.is_dir():
            shutil.copytree(src, target / folder)
    # Mandara-style includes + README for Pages docs discoverability
    includes = target / "_includes"
    includes.mkdir(exist_ok=True)
    (includes / "head-custom.html").write_text(
        """{% comment %}Super site subtree — keep orange brand; Mandara chrome stays on parent docs.{% endcomment %}
<meta name="color-scheme" content="light" />
<link rel="stylesheet" href="{{ '/super/site.css' | relative_url }}" />
<script src="{{ '/super/super-highlight.js' | relative_url }}" defer></script>
""",
        encoding="utf-8",
    )
    (target / "README.md").write_text(
        """# Super language site

Static GitHub Pages subtree for the Super programming language
(benchmarks, winners, charts, book, spec, examples). Generated from the Super repo:

```bash
cd /path/to/super/benchmarks
python3 report_page.py
# writes docs/ (GitHub Pages) and publishes into ../mandara/super
```

Canonical source tree in Super: `docs/` (Jekyll `_config.yml`, `_includes/`,
`_layouts/`, `_sass/`). `benchmarks/www` is a symlink to `docs/`.

Preview: open `index.html` or serve this folder. Parent Mandara Pages host:
`https://thehamdiz.github.io/mandara/super/`.

Regenerate after benchmark runs; do not hand-edit generated HTML.
""",
        encoding="utf-8",
    )
    # Soft link from Mandara docs index note — only if docs/README exists
    docs_readme = target_root / "docs" / "README.md"
    if docs_readme.is_file():
        marker = "<!-- super-site-link -->"
        body = docs_readme.read_text(encoding="utf-8")
        link_block = (
            f"\n{marker}\n## Super language\n\n"
            "The [Super language site](../super/) (benchmarks, book, examples) "
            "lives as a first-class Pages subtree under `/super/`.\n"
        )
        if marker not in body:
            docs_readme.write_text(body.rstrip() + link_block + "\n", encoding="utf-8")
    print(f"published Super site → {target}")
    return target


def render_html(report: dict[str, Any], path: Path) -> None:
    """Entry point used by runner.py: writes report + full site, syncs /tmp/bm_www."""
    render_site(report, path, sync=True)


if __name__ == "__main__":
    results = ROOT / "results.json"
    report = json.loads(results.read_text(encoding="utf-8")) if results.is_file() else {}
    render_site(report, ROOT / "report.html", sync=True)
    print(f"wrote {WWW / 'index.html'} and synced /tmp/bm_www")
