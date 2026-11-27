/**
 * Super Book instant pager — no 3D curl / drag spring.
 * Frozen ±10 page cache; click / keyboard / buttons swap fronts only.
 */
(function () {
  const reader = document.getElementById("reader");
  const openBtn = document.getElementById("open-book");
  const closeBtn = document.getElementById("close-reader");
  const prevBtn = document.getElementById("prev-page");
  const nextBtn = document.getElementById("next-page");
  const spread = document.getElementById("book-spread");
  const leftEl = document.getElementById("page-left");
  const rightEl = document.getElementById("page-right");
  const underLeft = document.getElementById("under-left");
  const underRight = document.getElementById("under-right");
  const progressEl = document.getElementById("reader-progress");
  const pageLabel = document.getElementById("page-label");
  if (!reader || !spread || !rightEl) return;

  const CACHE_RADIUS = 10;

  let pages = [];
  let idx = 0;
  const mountedIdx = Object.create(null);
  let prefetchTimer = 0;
  let prefetchGen = 0;

  const isPhone = () => window.matchMedia("(max-width: 900px)").matches;
  const step = () => (isPhone() ? 1 : 2);

  function pageAt(i) {
    if (i < 0 || i >= pages.length) return null;
    return pages[i];
  }

  function escapeText(s) {
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function face(el, which) {
    if (!el) return null;
    return el.querySelector(".face." + which) || el;
  }

  function pageKey(page, fallbackN) {
    if (!page) return "empty";
    return String(page.n || fallbackN || "") + ":" + (page.kind || "body");
  }

  // ── Immutable page cache ─────────────────────────────────────────────────

  const PageCache = {
    radius: CACHE_RADIUS,
    _map: new Map(),

    has(i) {
      return this._map.has(i);
    },
    size() {
      return this._map.size;
    },
    stats() {
      return {
        size: this._map.size,
        radius: this.radius,
        keys: Array.from(this._map.keys()).sort((a, b) => a - b),
      };
    },
    frozenHtml(i) {
      const e = this._map.get(i);
      return e ? e.frozenHtml : null;
    },
    getEntry(i) {
      return this._map.get(i) || null;
    },
    getTemplate(i) {
      const e = this._map.get(i);
      return e ? e.template : null;
    },

    ensure(i) {
      if (i < 0 || i >= pages.length) return null;
      const existing = this._map.get(i);
      if (existing) return existing;

      const page = pages[i];
      const template = document.createElement("div");
      template.className = "book-page-cache";
      template.setAttribute("data-cache-i", String(i));
      template.setAttribute("data-frozen", "1");
      if (page.html) {
        template.innerHTML = page.html;
      } else {
        const raw = (page.lines || []).join("\n");
        template.innerHTML = window.SuperHighlight
          ? window.SuperHighlight.renderPlainPage(raw)
          : escapeText(raw).replace(/\n/g, "<br>");
      }
      if (
        page.kind !== "cover" &&
        page.kind !== "section" &&
        window.SuperHighlight
      ) {
        window.SuperHighlight.run(template);
      }
      template.querySelectorAll("code").forEach((c) => {
        c.dataset.hl = "1";
      });

      const entry = {
        index: i,
        key: pageKey(page, i + 1),
        kind: page.kind || "body",
        num:
          page.kind === "cover" || page.kind === "section"
            ? ""
            : String(page.n || i + 1),
        template,
        frozenHtml: template.innerHTML,
      };
      Object.freeze(entry);
      this._map.set(i, entry);
      return entry;
    },

    dropOutside(_center) {},

    prefetchAround(center) {
      const gen = ++prefetchGen;
      const lo = Math.max(0, center - this.radius);
      const hi = Math.min(pages.length - 1, center + this.radius);
      const queue = [];
      for (let i = lo; i <= hi; i++) {
        if (!this._map.has(i)) queue.push(i);
      }
      queue.sort(
        (a, b) => Math.abs(a - center) - Math.abs(b - center) || a - b
      );
      const pump = () => {
        if (gen !== prefetchGen) return;
        for (let n = 0; n < 3 && queue.length; n++) {
          this.ensure(queue.shift());
        }
        if (queue.length) {
          if (typeof requestIdleCallback === "function") {
            prefetchTimer = requestIdleCallback(pump, { timeout: 120 });
          } else {
            prefetchTimer = setTimeout(pump, 0);
          }
        }
      };
      if (prefetchTimer) {
        clearTimeout(prefetchTimer);
        if (typeof cancelIdleCallback === "function") {
          try {
            cancelIdleCallback(prefetchTimer);
          } catch (_) {}
        }
      }
      pump();
    },
  };

  function attachCached(el, pageIndex, slot) {
    if (!el) return;
    const inner = el.querySelector(".page-inner");
    const num = el.querySelector(".page-num");
    if (!inner) return;

    if (pageIndex == null || pageIndex < 0 || pageIndex >= pages.length) {
      if (mountedIdx[slot] === -1 && !inner.childNodes.length) return;
      while (inner.firstChild) inner.removeChild(inner.firstChild);
      if (num) num.textContent = "";
      mountedIdx[slot] = -1;
      inner.removeAttribute("data-page-i");
      return;
    }

    if (
      mountedIdx[slot] === pageIndex &&
      inner.getAttribute("data-page-i") === String(pageIndex)
    ) {
      return;
    }

    const entry = PageCache.ensure(pageIndex);
    if (!entry) return;

    const clone = entry.template.cloneNode(true);
    while (inner.firstChild) inner.removeChild(inner.firstChild);
    while (clone.firstChild) inner.appendChild(clone.firstChild);
    if (num) num.textContent = entry.num;
    mountedIdx[slot] = pageIndex;
    inner.setAttribute("data-page-i", String(pageIndex));
    inner.setAttribute("data-frozen-html", "1");
    inner.classList.add("no-hl");

    const sheet = el.closest ? el.closest(".sheet") : null;
    const host = sheet || el;
    if (entry.kind === "cover" || entry.kind === "section") {
      host.classList.add("is-cover");
      if (entry.kind === "section") host.classList.add("section-face");
      else host.classList.remove("section-face");
    } else {
      host.classList.remove("is-cover", "section-face");
    }
  }

  function paintProgress() {
    const phone = isPhone();
    const total = Math.max(pages.length, 1);
    const shown = phone ? idx + 1 : Math.min(idx + 2, total);
    if (progressEl) progressEl.style.width = ((shown / total) * 100).toFixed(1) + "%";
    if (pageLabel) {
      pageLabel.textContent = phone
        ? idx + 1 + " / " + total
        : Math.min(idx + 1, total) + "–" + shown + " / " + total;
    }
  }

  function paintSpread() {
    const phone = isPhone();
    if (phone) {
      if (leftEl) leftEl.style.display = "none";
      if (underLeft) underLeft.style.display = "none";
      if (underRight) underRight.style.display = "none";
      attachCached(face(rightEl, "front") || rightEl, idx, "rf");
      // Clear unused faces (no animation buffers)
      attachCached(face(rightEl, "back"), null, "rb");
      attachCached(face(leftEl, "front"), null, "lf");
      attachCached(face(leftEl, "back"), null, "lb");
    } else {
      if (leftEl) leftEl.style.display = "";
      if (underLeft) underLeft.style.display = "none";
      if (underRight) underRight.style.display = "none";
      attachCached(face(leftEl, "front") || leftEl, idx, "lf");
      attachCached(face(rightEl, "front") || rightEl, idx + 1, "rf");
      attachCached(face(leftEl, "back"), null, "lb");
      attachCached(face(rightEl, "back"), null, "rb");
    }
    paintProgress();
    PageCache.prefetchAround(idx);
  }

  function canTurn(dir) {
    const next = idx + (dir > 0 ? step() : -step());
    return next >= 0 && next < pages.length;
  }

  /** Instant page change — no curl, no spring, no content rewrite of frozen pages. */
  function turn(dir) {
    if (!pages.length || !canTurn(dir)) return;
    const next = idx + (dir > 0 ? step() : -step());
    PageCache.ensure(next);
    if (!isPhone()) PageCache.ensure(next + 1);
    idx = next;
    paintSpread();
  }

  function openReader() {
    reader.classList.add("is-open");
    document.body.style.overflow = "hidden";
    paintSpread();
  }
  function closeReader() {
    reader.classList.remove("is-open");
    document.body.style.overflow = "";
  }

  function onSpreadClick(e) {
    if (e.target.closest && e.target.closest("a, button")) return;
    const rect = spread.getBoundingClientRect();
    const mid = rect.left + rect.width * 0.5;
    turn(e.clientX >= mid ? 1 : -1);
  }

  if (openBtn) openBtn.addEventListener("click", openReader);
  document.querySelectorAll("[data-open-book]").forEach((el) => {
    el.addEventListener("click", openReader);
  });
  if (closeBtn) closeBtn.addEventListener("click", closeReader);
  if (prevBtn) prevBtn.addEventListener("click", () => turn(-1));
  if (nextBtn) nextBtn.addEventListener("click", () => turn(1));
  spread.addEventListener("click", onSpreadClick);

  window.addEventListener("keydown", (e) => {
    if (!reader.classList.contains("is-open")) return;
    if (e.key === "ArrowRight" || e.key === "PageDown" || e.key === " ") {
      e.preventDefault();
      turn(1);
    } else if (e.key === "ArrowLeft" || e.key === "PageUp") {
      e.preventDefault();
      turn(-1);
    } else if (e.key === "Escape") {
      closeReader();
    }
  });

  window.addEventListener("resize", () => {
    if (reader.classList.contains("is-open")) paintSpread();
  });

  function normalizeHtml(s) {
    return String(s || "")
      .replace(/\s*data-cursor-ref="[^"]*"/g, "")
      .replace(/\s*data-cursor-ref='[^']*'/g, "");
  }

  async function assertMultiFlipIdentity(rounds) {
    rounds = rounds || 5;
    if (!reader.classList.contains("is-open")) openReader();
    const baselines = new Map();
    const failures = [];
    const record = (label) => {
      const indices = isPhone() ? [idx] : [idx, idx + 1];
      const hosts = isPhone()
        ? [face(rightEl, "front")]
        : [face(leftEl, "front"), face(rightEl, "front")];
      indices.forEach((i, n) => {
        if (i < 0 || i >= pages.length) return;
        const frozen = PageCache.frozenHtml(i);
        if (frozen == null) {
          failures.push({ label, i, reason: "missing frozenHtml" });
          return;
        }
        if (!baselines.has(i)) baselines.set(i, frozen);
        else if (baselines.get(i) !== frozen) {
          failures.push({ label, i, reason: "frozenHtml mutated" });
        }
        PageCache.ensure(i);
        if (PageCache.frozenHtml(i) !== baselines.get(i)) {
          failures.push({ label, i, reason: "ensure() rebuilt" });
        }
        const inner = hosts[n] && hosts[n].querySelector(".page-inner");
        if (!inner) return;
        if (inner.getAttribute("data-page-i") !== String(i)) {
          failures.push({ label, i, reason: "data-page-i mismatch" });
          return;
        }
        if (normalizeHtml(inner.innerHTML) !== normalizeHtml(frozen)) {
          failures.push({ label, i, reason: "live !== frozen" });
        }
      });
    };
    record("start");
    for (let r = 0; r < rounds; r++) {
      if (!canTurn(1)) break;
      turn(1);
      record("fwd-" + r);
    }
    for (let r = 0; r < rounds; r++) {
      if (!canTurn(-1)) break;
      turn(-1);
      record("back-" + r);
    }
    const ok = failures.length === 0;
    if (!ok) console.error("[book-pager] identity FAILED", failures);
    else console.info("[book-pager] identity OK", baselines.size, "pages");
    return { ok, failures, pagesTracked: baselines.size, rounds, cacheSize: PageCache.size() };
  }

  window.BookPageCache = PageCache;
  window.BookFlipDebug = {
    get idx() {
      return idx;
    },
    cacheStats() {
      return PageCache.stats();
    },
    frozenHtml(i) {
      return PageCache.frozenHtml(i);
    },
    assertMultiFlipIdentity,
    mode: "instant-pager",
  };

  fetch("book-pages.json")
    .then((r) => r.json())
    .then((data) => {
      pages = data.pages || [];
      idx = 0;
      for (let i = 0; i <= Math.min(pages.length - 1, CACHE_RADIUS); i++) {
        PageCache.ensure(i);
      }
      if (reader.classList.contains("is-open")) paintSpread();
    })
    .catch(() => {
      pages = [
        {
          n: 1,
          kind: "cover",
          html: '<article class="book-cover"><h1>Could not load book</h1></article>',
        },
      ];
      PageCache.ensure(0);
    });
})();
