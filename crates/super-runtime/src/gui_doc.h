/* Tab-separated widget document shared by the Win32 and Cocoa providers.
 *
 *   gui<TAB>1
 *   title<TAB>Title
 *   size<TAB>w<TAB>h
 *   node<TAB>kind<TAB>id<TAB>text<TAB>value<TAB>parent<TAB>cols
 *   item<TAB>id<TAB>text
 */
#ifndef SUPER_GUI_DOC_H
#define SUPER_GUI_DOC_H

#include <stdlib.h>
#include <string.h>

#define GD_NODES 256
#define GD_ITEMS 48

typedef struct GdNode {
  char *kind, *id, *text, *value, *parent;
  char *items[GD_ITEMS];
  int item_n, cols, selected;
  int x, y, w, h;
  void *widget;
} GdNode;

typedef struct GdDoc {
  GdNode nodes[GD_NODES];
  int n;
  char *title;
  int width, height;
} GdDoc;

static inline char *gd_dup(const char *s) {
  size_t n;
  char *p;
  if (!s) s = "";
  n = strlen(s);
  p = (char *)malloc(n + 1);
  if (!p) return 0;
  memcpy(p, s, n + 1);
  return p;
}

static inline void gd_node_free(GdNode *node) {
  int i;
  free(node->kind);
  free(node->id);
  free(node->text);
  free(node->value);
  free(node->parent);
  for (i = 0; i < node->item_n; i++) free(node->items[i]);
  memset(node, 0, sizeof *node);
}

static inline void gd_clear(GdDoc *doc) {
  int i;
  for (i = 0; i < doc->n; i++) gd_node_free(&doc->nodes[i]);
  free(doc->title);
  memset(doc, 0, sizeof *doc);
}

static inline int gd_tabs(char *line, char **fields, int max) {
  int n = 0;
  int got;
  if (max < 1) return 0;
  fields[n++] = line;
  for (char *s = line; *s; s++) {
    if (*s == '\t' && n < max) {
      *s = 0;
      fields[n++] = s + 1;
    }
  }
  got = n;
  while (n < max) fields[n++] = (char *)"";
  return got;
}

static inline GdNode *gd_by_id(GdDoc *doc, const char *id) {
  int i;
  if (!id || !id[0]) return 0;
  for (i = 0; i < doc->n; i++) {
    if (doc->nodes[i].id && strcmp(doc->nodes[i].id, id) == 0) return &doc->nodes[i];
  }
  return 0;
}

static inline void gd_parse(GdDoc *doc, const char *script) {
  char *buf = gd_dup(script ? script : "");
  char *cursor;
  if (!buf) return;
  gd_clear(doc);
  cursor = buf;
  while (*cursor) {
    char *line = cursor;
    char *nl = strchr(cursor, '\n');
    char *fields[8];
    if (nl) {
      *nl = 0;
      cursor = nl + 1;
    } else {
      cursor += strlen(cursor);
    }
    if (!line[0] || !strncmp(line, "gui\t", 4)) continue;
    gd_tabs(line, fields, 8);
    if (!strcmp(fields[0], "title")) {
      free(doc->title);
      doc->title = gd_dup(fields[1]);
    } else if (!strcmp(fields[0], "size")) {
      doc->width = atoi(fields[1]);
      doc->height = atoi(fields[2]);
    } else if (!strcmp(fields[0], "item")) {
      GdNode *node = gd_by_id(doc, fields[1]);
      if (node && node->item_n < GD_ITEMS) node->items[node->item_n++] = gd_dup(fields[2]);
    } else if (!strcmp(fields[0], "node") && doc->n < GD_NODES) {
      GdNode *node = &doc->nodes[doc->n];
      memset(node, 0, sizeof *node);
      node->kind = gd_dup(fields[1]);
      node->id = gd_dup(fields[2]);
      node->text = gd_dup(fields[3]);
      node->value = gd_dup(fields[4]);
      node->parent = gd_dup(fields[5]);
      node->cols = atoi(fields[6]);
      if (node->kind && !strcmp(node->kind, "chrome")) gd_node_free(node);
      else doc->n++;
    }
  }
  free(buf);
}

static inline int gd_box(const char *kind) {
  return kind && (!strcmp(kind, "column") || !strcmp(kind, "row") || !strcmp(kind, "panel") ||
                  !strcmp(kind, "grid") || !strcmp(kind, "scroll"));
}

static inline int gd_pref_h(const GdNode *node) {
  const char *kind = node->kind ? node->kind : "";
  const char *text = node->text ? node->text : "";
  const char *value = node->value ? node->value : "";
  if (!strcmp(kind, "list")) return 140;
  if (!strcmp(kind, "tabs")) return 36;
  if (!strcmp(kind, "dialog")) return value[0] ? 64 : 0;
  if (!strcmp(kind, "toast") || !strcmp(kind, "flash")) return text[0] ? 24 : 0;
  if (!strcmp(kind, "spacer") || !strcmp(kind, "shape") || !strcmp(kind, "separator")) return 12;
  if (!strcmp(kind, "slider") || !strcmp(kind, "progress")) return 28;
  if (gd_box(kind)) return 0;
  if (!text[0] && !value[0] && strcmp(kind, "button") && strcmp(kind, "input") &&
      strcmp(kind, "field") && strcmp(kind, "check") && strcmp(kind, "checkbox") &&
      strcmp(kind, "toggle") && strcmp(kind, "radio"))
    return 0;
  return 28;
}

static inline int gd_layout(GdDoc *doc, int idx, int x, int y, int w, int depth);

static inline int gd_kids(GdDoc *doc, int idx, int *out, int cap) {
  const char *id;
  int n = 0;
  int i;
  if (idx < 0 || idx >= doc->n) return 0;
  id = doc->nodes[idx].id ? doc->nodes[idx].id : "";
  if (!id[0]) return 0;
  for (i = 0; i < doc->n && n < cap; i++) {
    const char *parent = doc->nodes[i].parent ? doc->nodes[i].parent : "";
    if (i != idx && strcmp(parent, id) == 0) out[n++] = i;
  }
  return n;
}

static inline int gd_layout(GdDoc *doc, int idx, int x, int y, int w, int depth) {
  GdNode *node;
  int kids[GD_NODES];
  int nk, i, gap = 8, used = 0;
  const char *kind;
  if (depth > 24 || idx < 0 || idx >= doc->n || w < 1) return 0;
  node = &doc->nodes[idx];
  kind = node->kind ? node->kind : "";
  nk = gd_kids(doc, idx, kids, GD_NODES);
  node->x = x;
  node->y = y;
  node->w = w;
  if (!strcmp(kind, "row") && nk > 0) {
    int cw = (w - gap * (nk - 1)) / nk;
    int max_h = 0;
    int xx = x;
    if (cw < 1) cw = 1;
    for (i = 0; i < nk; i++) {
      int h = gd_layout(doc, kids[i], xx, y, cw, depth + 1);
      if (h > max_h) max_h = h;
      xx += cw + gap;
    }
    used = max_h;
  } else if (!strcmp(kind, "grid") && nk > 0) {
    int cols = node->cols > 0 ? node->cols : 1;
    int cw = (w - gap * (cols - 1)) / cols;
    int row_h = 0;
    int col = 0;
    int yy = y;
    if (cw < 1) cw = 1;
    for (i = 0; i < nk; i++) {
      int h = gd_layout(doc, kids[i], x + col * (cw + gap), yy, cw, depth + 1);
      if (h > row_h) row_h = h;
      col++;
      if (col >= cols) {
        yy += row_h + gap;
        used += row_h + (i + 1 < nk ? gap : 0);
        col = 0;
        row_h = 0;
      }
    }
    if (col) used += row_h;
  } else if (gd_box(kind) || nk > 0) {
    int yy = y + (!strcmp(kind, "panel") ? 8 : 0);
    int inner = w - (!strcmp(kind, "panel") ? 16 : 0);
    if (inner < 1) inner = 1;
    for (i = 0; i < nk; i++) {
      int h = gd_layout(doc, kids[i], x + (!strcmp(kind, "panel") ? 8 : 0), yy, inner, depth + 1);
      if (h <= 0) continue;
      yy += h + gap;
      used += h + gap;
    }
    if (used > 0) used -= gap;
    if (!strcmp(kind, "panel") && used > 0) used += 16;
  } else {
    used = gd_pref_h(node);
  }
  node->h = used;
  return used;
}

static inline void gd_layout_all(GdDoc *doc, int width, int height) {
  int y = 12;
  int i;
  int inner = width - 24;
  (void)height;
  if (inner < 1) inner = 1;
  for (i = 0; i < doc->n; i++) {
    const char *parent = doc->nodes[i].parent ? doc->nodes[i].parent : "";
    int h;
    if (parent[0]) continue;
    h = gd_layout(doc, i, 12, y, inner, 0);
    if (h > 0) y += h + 8;
  }
}

#endif
