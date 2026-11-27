---
layout: page
title: Site guide
nav: guide
description: How the Super Pages site is built and regenerated.
permalink: /guide.html
---

The public Super site lives in this `docs/` tree — the GitHub Pages source for
the language homepage (benchmarks, winners, charts, book, examples, install).

## Regenerate after a bench run

```bash
cd benchmarks
python3 report_page.py
# writes docs/ (this tree), syncs /tmp/bm_www, publishes ../mandara/super
```

Or from the repo root:

```bash
./scripts/publish_site.sh
```

## Local preview

```bash
# Static (no Jekyll) — generated HTML is enough:
python3 -m http.server 8765 --bind 127.0.0.1 --directory docs

# Or the live sync mirror:
python3 -m http.server 8765 --bind 127.0.0.1 --directory /tmp/bm_www

# Full Jekyll (requires Bundler):
cd docs && bundle install && bundle exec jekyll serve --baseurl ""
```

## Layout

| Path | Role |
|------|------|
| `_config.yml` | Pages / remote_theme / plugins |
| `_includes/` | head, nav, footer for Markdown pages |
| `_layouts/` | default + page |
| `_sass/` | Jekyll layer tokens |
| `index.html` / `report.html` | Live scoreboard (generator) |
| `winners.html` | Category / overall winners |
| `charts.html` | Composite chart focus |
| `book.html` | Flip book reader |
| `site.css` | Brand stylesheet (generator) |
