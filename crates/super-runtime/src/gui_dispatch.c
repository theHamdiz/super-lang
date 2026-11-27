/* Picks the desktop provider.
 *
 * Linux: GTK 4, Wayland before X11.
 * Windows: Win32, then GTK if Win32 cannot open.
 * macOS: Cocoa, then GTK if Cocoa cannot open.
 * SUPER_GUI=gtk|wayland|x11|win32|cocoa|macos forces that provider first.
 */
#ifndef SUPER_GUI_PASTED
#define SUPER_GUI_SPLIT 1
#ifndef SUPER_PROVIDER_API
#define SUPER_PROVIDER_API static
#endif
#include "gtk_provider.c"
#include "win32_provider.c"
#include "cocoa_provider.c"
#endif

#include <stdlib.h>
#include <string.h>

#undef super_provider_open
#undef super_provider_present
#undef super_provider_pump
#undef super_provider_quit
#undef super_provider_live
#undef super_provider_backend
#undef super_provider_take_key
#undef super_provider_take_click
#undef super_provider_mouse
#undef super_provider_value
#undef super_provider_close

static const SuperGuiFns super_gtk_fns = {
    "gtk",
    super_gtk_open,
    super_gtk_present,
    super_gtk_pump,
    super_gtk_quit,
    super_gtk_live,
    super_gtk_backend,
    super_gtk_take_key,
    super_gtk_take_click,
    super_gtk_mouse,
    super_gtk_value,
    super_gtk_close,
};

static const SuperGuiFns *sp_used;

static int sp_is(const char *value, const char *name) { return value && strcmp(value, name) == 0; }

static int sp_wants_gtk(const char *value) {
  return sp_is(value, "gtk") || sp_is(value, "wayland") || sp_is(value, "x11");
}

static const SuperGuiFns *sp_preferred(void) {
  const char *value = getenv("SUPER_GUI");
  if (sp_wants_gtk(value)) return &super_gtk_fns;
  if (sp_is(value, "win32")) return &super_win32_fns;
  if (sp_is(value, "cocoa") || sp_is(value, "macos")) return &super_cocoa_fns;
#if defined(_WIN32)
  return &super_win32_fns;
#elif defined(__APPLE__)
  return &super_cocoa_fns;
#else
  return &super_gtk_fns;
#endif
}

static int sp_try(const SuperGuiFns *fns, int w, int h, const char *title) {
  if (!fns || !fns->open || !fns->open(w, h, title)) return 0;
  sp_used = fns;
  return 1;
}

int super_provider_open(int w, int h, const char *title) {
  const SuperGuiFns *order[3];
  int i, n = 0;
  const SuperGuiFns *first = sp_preferred();
  if (sp_used && sp_used->live && sp_used->live()) return 1;
  order[n++] = first;
  if (first != &super_gtk_fns) order[n++] = &super_gtk_fns;
  if (first != &super_win32_fns) order[n++] = &super_win32_fns;
  if (first != &super_cocoa_fns) order[n++] = &super_cocoa_fns;
  for (i = 0; i < n; i++) {
    if (sp_try(order[i], w, h, title)) return 1;
  }
  sp_used = 0;
  return 0;
}

void super_provider_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  if (sp_used && sp_used->present) sp_used->present(w, h, script, px, pw, ph);
}

int super_provider_pump(void) {
  if (!sp_used || !sp_used->pump) return 0;
  return sp_used->pump();
}

int super_provider_quit(void) {
  if (!sp_used || !sp_used->quit) return 0;
  return sp_used->quit();
}

int super_provider_live(void) {
  if (!sp_used || !sp_used->live) return 0;
  return sp_used->live();
}

const char *super_provider_backend(void) {
  if (sp_used && sp_used->backend) return sp_used->backend();
  return sp_preferred()->name;
}

int super_provider_take_key(char *buf, int cap) {
  if (!sp_used || !sp_used->take_key) {
    if (buf && cap > 0) buf[0] = 0;
    return 0;
  }
  return sp_used->take_key(buf, cap);
}

int super_provider_take_click(char *buf, int cap) {
  if (!sp_used || !sp_used->take_click) {
    if (buf && cap > 0) buf[0] = 0;
    return 0;
  }
  return sp_used->take_click(buf, cap);
}

void super_provider_mouse(int *x, int *y, int *down) {
  if (!sp_used || !sp_used->mouse) {
    if (x) *x = 0;
    if (y) *y = 0;
    if (down) *down = 0;
    return;
  }
  sp_used->mouse(x, y, down);
}

int super_provider_value(const char *id, char *buf, int cap) {
  if (!sp_used || !sp_used->value) {
    if (buf && cap > 0) buf[0] = 0;
    return 0;
  }
  return sp_used->value(id, buf, cap);
}

void super_provider_close(void) {
  if (sp_used && sp_used->close) sp_used->close();
  sp_used = 0;
}
