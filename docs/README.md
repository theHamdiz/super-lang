# Super language site (GitHub Pages)

Canonical marketing / benchmark site for Super. This `docs/` folder is the
**GitHub Pages** source (Jekyll + generated HTML).

**Stage-1 (Protocol 2.0.0):** Super AOT composite **90.06 #1** (Speed/Mem/LoC 100; Cycle → Super JIT). See `REPORT.md`, suite aggregates, `stage1_speed_walls.md`.

## Tree

| Path | Role |
|------|------|
| `_config.yml` | Pages config, `remote_theme: pages-themes/primer`, plugins |
| `_includes/` | `head.html`, `nav.html`, `footer.html` |
| `_layouts/` | `default.html`, `page.html` |
| `_sass/` | Product tokens + thin Jekyll layer |
| `assets/` | Fonts, JS mirrors, compiled CSS |
| `index.html` / `report.html` | Live scoreboard (generator) |
| `winners.html` / `charts.html` | Focus pages (generator) |
| `book.html`, `language.html`, … | Product pages (generator) |
| `guide.md` | Jekyll layout demo / operator guide |
| `site.css` | Brand stylesheet (owned by `report_page.py`) |

Legacy path: `benchmarks/www` → symlink to this directory.

## Regenerate (keeps bench runner working)

```bash
cd benchmarks
python3 report_page.py
# → docs/, /tmp/bm_www, and ../mandara/super
```

```bash
./scripts/publish_site.sh
```

`runner.py --rescore` still calls `render_html` → same pipeline.

## Serve / build for Pages

```bash
# Static preview (no additional site runtime required)
python3 -m http.server 8765 --bind 127.0.0.1 --directory docs

# Jekyll (GitHub Pages parity; needs Bundler)
cd docs
bundle install
bundle exec jekyll serve --baseurl ""
# open http://127.0.0.1:4000/
```

On GitHub: Settings → Pages → Source = Deploy from branch → `/docs` folder.

Project URL: `https://thehamdiz.github.io/super-lang/` (`baseurl:
/super-lang` in `_config.yml`).
