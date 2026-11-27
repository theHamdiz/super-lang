(function () {
  const KEYWORDS =
    "when ensure else for each while loop break continue in return fail panic or err test if state await scope spawn parallel par chan shared after use as mod get post put delete rewrite trait embed true false none type Code";
  const TYPES =
    "i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool str void never";
  const kwRe = new RegExp("^(?:" + KEYWORDS.trim().split(/\s+/).join("|") + ")$");
  const tyRe = new RegExp("^(?:" + TYPES.trim().split(/\s+/).join("|") + ")$");

  function esc(s) {
    return s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function span(cls, text) {
    return '<span class="' + cls + '">' + esc(text) + "</span>";
  }

  function highlightSr(src) {
    let out = "";
    let i = 0;
    const n = src.length;
    while (i < n) {
      if (src[i] === "/" && src[i + 1] === "/") {
        let j = i;
        while (j < n && src[j] !== "\n") j++;
        out += span("tok-comment", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === "/" && src[i + 1] === "*") {
        let j = i + 2;
        while (j < n && !(src[j] === "*" && src[j + 1] === "/")) j++;
        j = Math.min(n, j + 2);
        out += span("tok-comment", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === '"') {
        let j = i + 1;
        while (j < n) {
          if (src[j] === "\\") {
            j += 2;
            continue;
          }
          if (src[j] === '"') {
            j++;
            break;
          }
          j++;
        }
        out += span("tok-string", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === "#" && /[A-Za-z_]/.test(src[i + 1] || "")) {
        let j = i + 1;
        while (j < n && /[A-Za-z0-9_]/.test(src[j])) j++;
        out += span("tok-macro", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === "@") {
        let j = i + 1;
        while (j < n && /[A-Za-z0-9_]/.test(src[j])) j++;
        out += span("tok-mut", src.slice(i, j));
        i = j;
        continue;
      }
      if ((src[i] === "+" || src[i] === "-") && /[A-Za-z_]/.test(src[i + 1] || "")) {
        const prev = i === 0 ? "\n" : src[i - 1];
        if (/[\s{\[(,;=]/.test(prev) || i === 0) {
          out += span("tok-vis", src[i]);
          i++;
          continue;
        }
      }
      // Range operator `..` (and `..=` if present) — must win over number scans
      // so `0..len(xs)` is never swallowed as a single numeric token.
      if (src[i] === "." && src[i + 1] === ".") {
        let j = i + 2;
        if (src[j] === "=") j++;
        out += span("tok-op", src.slice(i, j));
        i = j;
        continue;
      }
      if (/[0-9]/.test(src[i]) || (src[i] === "." && /[0-9]/.test(src[i + 1] || ""))) {
        let j = i;
        // Hex / binary / octal: only those prefixes may consume a-f letters.
        if (src[j] === "0" && /[xboXBO]/.test(src[j + 1] || "")) {
          const base = src[j + 1].toLowerCase();
          j += 2;
          const isDigit =
            base === "x"
              ? (c) => /[0-9a-fA-F]/.test(c)
              : base === "b"
                ? (c) => c === "0" || c === "1"
                : (c) => /[0-7]/.test(c);
          while (j < n) {
            if (src[j] === "_") {
              j++;
              continue;
            }
            if (isDigit(src[j])) j++;
            else break;
          }
        } else {
          // Decimal int / float. Stop before `..` so ranges stay intact.
          if (src[j] === ".") j++;
          while (j < n && /[0-9_]/.test(src[j])) j++;
          if (
            src[j] === "." &&
            src[j + 1] !== "." &&
            /[0-9]/.test(src[j + 1] || "")
          ) {
            j++;
            while (j < n && /[0-9_]/.test(src[j])) j++;
          }
          if (/[eE]/.test(src[j] || "") && /[+\-0-9]/.test(src[j + 1] || "")) {
            j++;
            if (/[+-]/.test(src[j] || "")) j++;
            while (j < n && /[0-9_]/.test(src[j])) j++;
          }
        }
        out += span("tok-number", src.slice(i, j));
        i = j;
        continue;
      }
      if (/[A-Za-z_]/.test(src[i])) {
        let j = i + 1;
        while (j < n && /[A-Za-z0-9_]/.test(src[j])) j++;
        const word = src.slice(i, j);
        // After an ident, if `..` follows, the range end is a full postfix
        // expression (call / field / index / parens) tokenized normally below.
        if (kwRe.test(word)) out += span("tok-kw", word);
        else if (tyRe.test(word)) out += span("tok-type", word);
        else if (/^[A-Z]/.test(word)) out += span("tok-type", word);
        else out += esc(word);
        i = j;
        continue;
      }
      out += esc(src[i]);
      i++;
    }
    return out;
  }

  /** Light generic highlighter for bash / text / ebnf / rust / etc. */
  function highlightGeneric(src, lang) {
    if (lang === "sr" || lang === "super") return highlightSr(src);
    let out = "";
    let i = 0;
    const n = src.length;
    while (i < n) {
      if (src[i] === "#" && (lang === "bash" || lang === "sh" || lang === "shell" || lang === "zsh")) {
        let j = i;
        while (j < n && src[j] !== "\n") j++;
        out += span("tok-comment", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === "/" && src[i + 1] === "/" && (lang === "rust" || lang === "c" || lang === "js" || lang === "ts")) {
        let j = i;
        while (j < n && src[j] !== "\n") j++;
        out += span("tok-comment", src.slice(i, j));
        i = j;
        continue;
      }
      if (src[i] === '"' || src[i] === "'") {
        const q = src[i];
        let j = i + 1;
        while (j < n) {
          if (src[j] === "\\") {
            j += 2;
            continue;
          }
          if (src[j] === q) {
            j++;
            break;
          }
          j++;
        }
        out += span("tok-string", src.slice(i, j));
        i = j;
        continue;
      }
      if (/[0-9]/.test(src[i])) {
        let j = i;
        while (j < n && /[0-9a-fxA-F_.]/.test(src[j])) j++;
        out += span("tok-number", src.slice(i, j));
        i = j;
        continue;
      }
      out += esc(src[i]);
      i++;
    }
    return out;
  }

  function inlineMd(text) {
    // bold, italic, links, inline ticks — order matters
    let s = esc(text);
    s = s.replace(/`([^`\n]+)`/g, function (_, code) {
      return '<code class="sr-inline">' + highlightSr(code) + "</code>";
    });
    s = s.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');
    s = s.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
    s = s.replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, "$1<em>$2</em>");
    return s;
  }

  /** Render a plain-text / light-markdown page body to HTML. */
  function renderPlainPage(raw) {
    const lines = String(raw || "").split("\n");
    const parts = [];
    let i = 0;
    let listBuf = [];
    function flushList() {
      if (!listBuf.length) return;
      parts.push("<ul>" + listBuf.map((t) => "<li>" + inlineMd(t) + "</li>").join("") + "</ul>");
      listBuf = [];
    }
    while (i < lines.length) {
      const line = lines[i];
      const fence = line.match(/^```(\w+)?\s*$/);
      if (fence) {
        flushList();
        const lang = (fence[1] || "sr").toLowerCase();
        i++;
        const buf = [];
        while (i < lines.length && !/^```/.test(lines[i])) {
          buf.push(lines[i]);
          i++;
        }
        if (i < lines.length) i++;
        const code = buf.join("\n");
        const cls =
          lang === "sr" || lang === "super"
            ? "language-sr"
            : "language-" + lang;
        const body =
          lang === "sr" || lang === "super"
            ? highlightSr(code)
            : highlightGeneric(code, lang);
        const preCls = lang === "sr" || lang === "super" ? "sr-block" : "code-block";
        parts.push('<pre class="' + preCls + '"><code class="' + cls + '" data-hl="1">' + body + "</code></pre>");
        continue;
      }
      if (/^\s*[-*•]\s+/.test(line)) {
        listBuf.push(line.replace(/^\s*[-*•]\s+/, ""));
        i++;
        continue;
      }
      flushList();
      if (/^#{1,3}\s+/.test(line)) {
        const level = line.match(/^(#{1,3})/)[1].length;
        parts.push("<h" + level + ">" + inlineMd(line.replace(/^#{1,3}\s+/, "")) + "</h" + level + ">");
      } else if (/^\s*$/.test(line)) {
        // skip blank — paragraphs handle spacing
      } else {
        parts.push("<p>" + inlineMd(line) + "</p>");
      }
      i++;
    }
    flushList();
    return parts.join("\n") || "<p></p>";
  }

  function highlightInlineTicks(root) {
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    nodes.forEach((node) => {
      const parent = node.parentElement;
      if (!parent) return;
      if (parent.closest("pre, code, script, style, .tok-kw, .no-hl")) return;
      const text = node.nodeValue;
      if (!text || text.indexOf("`") === -1) return;
      const frag = document.createDocumentFragment();
      const re = /`([^`\n]+)`/g;
      let last = 0;
      let m;
      while ((m = re.exec(text))) {
        if (m.index > last) frag.appendChild(document.createTextNode(text.slice(last, m.index)));
        const code = document.createElement("code");
        code.className = "sr-inline";
        code.innerHTML = highlightSr(m[1]);
        frag.appendChild(code);
        last = m.index + m[0].length;
      }
      if (last === 0) return;
      if (last < text.length) frag.appendChild(document.createTextNode(text.slice(last)));
      parent.replaceChild(frag, node);
    });
  }

  function langOf(el) {
    const cls = el.className || "";
    const m = cls.match(/language-([a-z0-9_+-]+)/i);
    if (m) return m[1].toLowerCase();
    if (el.classList.contains("sr")) return "sr";
    return "sr";
  }

  function run(root) {
    root = root || document;
    root.querySelectorAll("pre code, code.language-sr, code.sr, code.language-super, code.sr-inline").forEach((el) => {
      if (el.dataset.hl === "1") return;
      const raw = el.textContent || "";
      const lang = el.classList.contains("sr-inline") ? "sr" : langOf(el);
      el.innerHTML = highlightGeneric(raw, lang === "super" ? "sr" : lang);
      el.dataset.hl = "1";
      if (lang === "sr" || lang === "super" || el.classList.contains("sr-inline")) {
        el.classList.add(el.classList.contains("sr-inline") ? "sr-inline" : "language-sr");
      }
      if (el.parentElement && el.parentElement.tagName === "PRE") {
        el.parentElement.classList.add(
          lang === "sr" || lang === "super" ? "sr-block" : "code-block"
        );
      }
    });
    const scope = root.querySelector
      ? root.querySelector(".spec-body, .page-inner, .book-body") || root
      : root;
    highlightInlineTicks(scope);
  }

  const api = {
    highlightSr: highlightSr,
    highlightGeneric: highlightGeneric,
    renderPlainPage: renderPlainPage,
    inlineMd: inlineMd,
    run: run,
  };
  if (typeof module !== "undefined" && module.exports) {
    module.exports = api;
  }
  if (typeof globalThis !== "undefined") {
    globalThis.SuperHighlight = api;
  }
  if (typeof window !== "undefined") {
    window.SuperHighlight = api;
    if (typeof document !== "undefined") {
      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", () => run());
      } else {
        run();
      }
    }
  }
})();
