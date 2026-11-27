/* GTK 4 window provider.
 *
 * Linux tries Wayland before X11. SUPER_GUI=wayland or SUPER_GUI=x11 forces
 * that GDK backend. Windows and macOS keep this provider for SUPER_GUI=gtk;
 * their default providers are Win32 and Cocoa. An explicit GDK_BACKEND is left
 * alone.
 *
 * A script that starts with "gui\t1" is a widget document and becomes real
 * GTK controls (boxes, buttons, entries, checks, scales, lists, notebooks).
 * Any other script is a pixel canvas inside the same GTK window, used by
 * games. The toolkit is loaded with dlopen, so a host without GTK stays
 * headless instead of failing the link.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#include <windows.h>
static void *sp_dlopen(const char *name) { return (void *)LoadLibraryA(name); }
static void *sp_dlsym(void *lib, const char *name) {
  return lib ? (void *)GetProcAddress((HMODULE)lib, name) : 0;
}
#elif defined(__wasm__) || defined(__wasi__)
static void *sp_dlopen(const char *name) {
  (void)name;
  return 0;
}
static void *sp_dlsym(void *lib, const char *name) {
  (void)lib;
  (void)name;
  return 0;
}
#else
#include <dlfcn.h>
static void *sp_dlopen(const char *name) { return dlopen(name, RTLD_NOW | RTLD_LOCAL); }
static void *sp_dlsym(void *lib, const char *name) { return lib ? dlsym(lib, name) : 0; }
#endif

#ifndef SUPER_PROVIDER_API
#define SUPER_PROVIDER_API static
#endif

#ifdef SUPER_GUI_SPLIT
#define super_provider_open super_gtk_open
#define super_provider_present super_gtk_present
#define super_provider_pump super_gtk_pump
#define super_provider_quit super_gtk_quit
#define super_provider_live super_gtk_live
#define super_provider_backend super_gtk_backend
#define super_provider_take_key super_gtk_take_key
#define super_provider_take_click super_gtk_take_click
#define super_provider_mouse super_gtk_mouse
#define super_provider_value super_gtk_value
#define super_provider_close super_gtk_close
#endif

typedef void (*SpCb)(void);
typedef void (*SpDraw)(void *, void *, int, int, void *);
typedef void (*SpDestroy)(void *, void *);

static struct {
  void (*gdk_set_allowed_backends)(const char *);
  void *(*gdk_display_get_default)(void);
  const char *(*gdk_display_get_name)(void *);
  void (*gtk_init)(void);
  void *(*gtk_window_new)(void);
  void (*gtk_window_set_title)(void *, const char *);
  void (*gtk_window_set_default_size)(void *, int, int);
  void (*gtk_window_present)(void *);
  void (*gtk_window_destroy)(void *);
  void (*gtk_window_set_child)(void *, void *);
  void (*gtk_window_set_titlebar)(void *, void *);
  void *(*gtk_header_bar_new)(void);
  void (*gtk_header_bar_set_title_widget)(void *, void *);
  void (*gtk_header_bar_set_show_title_buttons)(void *, int);
  void *(*gtk_box_new)(int, int);
  void (*gtk_box_append)(void *, void *);
  void *(*gtk_label_new)(const char *);
  void (*gtk_label_set_text)(void *, const char *);
  void (*gtk_label_set_wrap)(void *, int);
  void (*gtk_label_set_xalign)(void *, float);
  void (*gtk_widget_add_css_class)(void *, const char *);
  void *(*gtk_button_new_with_label)(const char *);
  void *(*gtk_entry_new)(void);
  void (*gtk_entry_set_placeholder_text)(void *, const char *);
  void (*gtk_editable_set_text)(void *, const char *);
  const char *(*gtk_editable_get_text)(void *);
  void *(*gtk_check_button_new_with_label)(const char *);
  void (*gtk_check_button_set_active)(void *, int);
  int (*gtk_check_button_get_active)(void *);
  void (*gtk_check_button_set_group)(void *, void *);
  void *(*gtk_scale_new_with_range)(int, double, double, double);
  void (*gtk_range_set_value)(void *, double);
  double (*gtk_range_get_value)(void *);
  void *(*gtk_progress_bar_new)(void);
  void (*gtk_progress_bar_set_fraction)(void *, double);
  void *(*gtk_scrolled_window_new)(void);
  void (*gtk_scrolled_window_set_child)(void *, void *);
  void (*gtk_scrolled_window_set_policy)(void *, int, int);
  void *(*gtk_list_box_new)(void);
  void (*gtk_list_box_append)(void *, void *);
  int (*gtk_list_box_row_get_index)(void *);
  void *(*gtk_notebook_new)(void);
  int (*gtk_notebook_append_page)(void *, void *, void *);
  void *(*gtk_frame_new)(const char *);
  void (*gtk_frame_set_child)(void *, void *);
  void *(*gtk_grid_new)(void);
  void (*gtk_grid_attach)(void *, void *, int, int, int, int);
  void (*gtk_grid_set_column_spacing)(void *, int);
  void (*gtk_grid_set_row_spacing)(void *, int);
  void *(*gtk_separator_new)(int);
  void *(*gtk_drawing_area_new)(void);
  void (*gtk_drawing_area_set_content_width)(void *, int);
  void (*gtk_drawing_area_set_content_height)(void *, int);
  void (*gtk_drawing_area_set_draw_func)(void *, SpDraw, void *, void (*)(void *));
  void *(*gtk_event_controller_key_new)(void);
  void *(*gtk_event_controller_motion_new)(void);
  void *(*gtk_gesture_click_new)(void);
  void (*gtk_widget_add_controller)(void *, void *);
  void (*gtk_widget_set_hexpand)(void *, int);
  void (*gtk_widget_set_vexpand)(void *, int);
  void (*gtk_widget_set_margin_top)(void *, int);
  void (*gtk_widget_set_margin_bottom)(void *, int);
  void (*gtk_widget_set_margin_start)(void *, int);
  void (*gtk_widget_set_margin_end)(void *, int);
  void (*gtk_widget_set_size_request)(void *, int, int);
  void (*gtk_widget_queue_draw)(void *);
  void (*gtk_widget_set_focusable)(void *, int);
  int (*gtk_widget_grab_focus)(void *);
  int (*g_main_context_iteration)(void *, int);
  unsigned long (*g_signal_connect_data)(void *, const char *, SpCb, void *, SpDestroy, unsigned);
  void *(*cairo_image_surface_create_for_data)(unsigned char *, int, int, int, int);
  void (*cairo_set_source_surface)(void *, void *, double, double);
  void (*cairo_paint)(void *);
  void (*cairo_surface_destroy)(void *);
  void (*cairo_scale)(void *, double, double);
  void *(*cairo_get_source)(void *);
  void (*cairo_pattern_set_filter)(void *, int);
  void (*cairo_save)(void *);
  void (*cairo_restore)(void *);
} sp;

#define SP_NODES 256
#define SP_ITEMS 48
#define SP_Q 64

typedef struct SpNode {
  char *kind, *id, *text, *value, *parent;
  char *items[SP_ITEMS];
  int item_n, cols, cursor, selected;
  void *widget, *box;
} SpNode;

static void *sp_libs[8];
static int sp_nlibs;
static int sp_ready, sp_inited, sp_widgets, sp_quit, sp_scale, sp_focused;
static int sp_mx, sp_my, sp_down, sp_nn, sp_cw, sp_ch, sp_pw, sp_ph;
static int sp_saw_enter, sp_kn, sp_kr, sp_cn, sp_cr;
static void *sp_win, *sp_root, *sp_canvas, *sp_heading;
static char *sp_script;
static char *sp_keys[SP_Q], *sp_clicks[SP_Q];
static uint32_t *sp_px;
static const char *sp_backend = "headless";
static SpNode sp_nodes[SP_NODES];

static char *sp_dup(const char *s) {
  size_t n;
  char *p;
  if (!s) s = "";
  n = strlen(s);
  p = (char *)malloc(n + 1);
  if (!p) return 0;
  memcpy(p, s, n + 1);
  return p;
}

static void *sp_find(const char *name) {
  int i;
  for (i = 0; i < sp_nlibs; i++) {
    void *sym = sp_dlsym(sp_libs[i], name);
    if (sym) return sym;
  }
  return 0;
}

static int sp_bind(void **dest, const char *name) {
  void *sym = sp_find(name);
  if (!sym) {
    fprintf(stderr, "gui: GTK is missing `%s`\n", name);
    return 0;
  }
  memcpy(dest, &sym, sizeof(void *));
  return 1;
}

static int sp_open_libs(void) {
#if defined(_WIN32)
  const char *gtk[] = {"gtk-4-1.dll", "libgtk-4-1.dll", 0};
  const char *gdk[] = {"gdk-4-1.dll", 0};
  const char *glib[] = {"glib-2.0-0.dll", "libglib-2.0-0.dll", 0};
  const char *gobj[] = {"gobject-2.0-0.dll", "libgobject-2.0-0.dll", 0};
  const char *cairo[] = {"cairo-2.dll", "libcairo-2.dll", 0};
#elif defined(__APPLE__)
  const char *gtk[] = {"libgtk-4.1.dylib", "libgtk-4.dylib",
                       "/opt/homebrew/lib/libgtk-4.1.dylib",
                       "/usr/local/lib/libgtk-4.1.dylib", 0};
  const char *gdk[] = {"libgdk-4.1.dylib", "/opt/homebrew/lib/libgdk-4.1.dylib", 0};
  const char *glib[] = {"libglib-2.0.0.dylib", "/opt/homebrew/lib/libglib-2.0.0.dylib", 0};
  const char *gobj[] = {"libgobject-2.0.0.dylib", "/opt/homebrew/lib/libgobject-2.0.0.dylib", 0};
  const char *cairo[] = {"libcairo.2.dylib", "/opt/homebrew/lib/libcairo.2.dylib", 0};
#else
  const char *gtk[] = {"libgtk-4.so.1", "libgtk-4.so", 0};
  const char *gdk[] = {"libgdk-4.so.1", "libgdk-4.so", 0};
  const char *glib[] = {"libglib-2.0.so.0", "libglib-2.0.so", 0};
  const char *gobj[] = {"libgobject-2.0.so.0", "libgobject-2.0.so", 0};
  const char *cairo[] = {"libcairo.so.2", "libcairo.so", 0};
#endif
  const char **groups[] = {gtk, gdk, glib, gobj, cairo};
  int g;
  sp_nlibs = 0;
  for (g = 0; g < 5; g++) {
    int i;
    for (i = 0; groups[g][i]; i++) {
      void *lib = sp_dlopen(groups[g][i]);
      if (lib && sp_nlibs < 8) {
        sp_libs[sp_nlibs++] = lib;
        break;
      }
    }
  }
  return sp_nlibs > 0;
}

static int sp_load(void) {
  if (sp_ready) return 1;
  if (!sp_open_libs()) {
    fprintf(stderr, "gui: GTK4 was not found, window stayed headless\n");
    return 0;
  }
#define SP_REQ(field, name) if (!sp_bind((void **)&sp.field, name)) return 0
  SP_REQ(gdk_set_allowed_backends, "gdk_set_allowed_backends");
  SP_REQ(gdk_display_get_default, "gdk_display_get_default");
  SP_REQ(gdk_display_get_name, "gdk_display_get_name");
  SP_REQ(gtk_init, "gtk_init");
  SP_REQ(gtk_window_new, "gtk_window_new");
  SP_REQ(gtk_window_set_title, "gtk_window_set_title");
  SP_REQ(gtk_window_set_default_size, "gtk_window_set_default_size");
  SP_REQ(gtk_window_present, "gtk_window_present");
  SP_REQ(gtk_window_destroy, "gtk_window_destroy");
  SP_REQ(gtk_window_set_child, "gtk_window_set_child");
  SP_REQ(gtk_window_set_titlebar, "gtk_window_set_titlebar");
  SP_REQ(gtk_header_bar_new, "gtk_header_bar_new");
  SP_REQ(gtk_header_bar_set_title_widget, "gtk_header_bar_set_title_widget");
  SP_REQ(gtk_header_bar_set_show_title_buttons, "gtk_header_bar_set_show_title_buttons");
  SP_REQ(gtk_box_new, "gtk_box_new");
  SP_REQ(gtk_box_append, "gtk_box_append");
  SP_REQ(gtk_label_new, "gtk_label_new");
  SP_REQ(gtk_label_set_text, "gtk_label_set_text");
  SP_REQ(gtk_label_set_wrap, "gtk_label_set_wrap");
  SP_REQ(gtk_label_set_xalign, "gtk_label_set_xalign");
  SP_REQ(gtk_widget_add_css_class, "gtk_widget_add_css_class");
  SP_REQ(gtk_button_new_with_label, "gtk_button_new_with_label");
  SP_REQ(gtk_entry_new, "gtk_entry_new");
  SP_REQ(gtk_entry_set_placeholder_text, "gtk_entry_set_placeholder_text");
  SP_REQ(gtk_editable_set_text, "gtk_editable_set_text");
  SP_REQ(gtk_editable_get_text, "gtk_editable_get_text");
  SP_REQ(gtk_check_button_new_with_label, "gtk_check_button_new_with_label");
  SP_REQ(gtk_check_button_set_active, "gtk_check_button_set_active");
  SP_REQ(gtk_check_button_get_active, "gtk_check_button_get_active");
  SP_REQ(gtk_check_button_set_group, "gtk_check_button_set_group");
  SP_REQ(gtk_scale_new_with_range, "gtk_scale_new_with_range");
  SP_REQ(gtk_range_set_value, "gtk_range_set_value");
  SP_REQ(gtk_range_get_value, "gtk_range_get_value");
  SP_REQ(gtk_progress_bar_new, "gtk_progress_bar_new");
  SP_REQ(gtk_progress_bar_set_fraction, "gtk_progress_bar_set_fraction");
  SP_REQ(gtk_scrolled_window_new, "gtk_scrolled_window_new");
  SP_REQ(gtk_scrolled_window_set_child, "gtk_scrolled_window_set_child");
  SP_REQ(gtk_scrolled_window_set_policy, "gtk_scrolled_window_set_policy");
  SP_REQ(gtk_list_box_new, "gtk_list_box_new");
  SP_REQ(gtk_list_box_append, "gtk_list_box_append");
  SP_REQ(gtk_list_box_row_get_index, "gtk_list_box_row_get_index");
  SP_REQ(gtk_notebook_new, "gtk_notebook_new");
  SP_REQ(gtk_notebook_append_page, "gtk_notebook_append_page");
  SP_REQ(gtk_frame_new, "gtk_frame_new");
  SP_REQ(gtk_frame_set_child, "gtk_frame_set_child");
  SP_REQ(gtk_grid_new, "gtk_grid_new");
  SP_REQ(gtk_grid_attach, "gtk_grid_attach");
  SP_REQ(gtk_grid_set_column_spacing, "gtk_grid_set_column_spacing");
  SP_REQ(gtk_grid_set_row_spacing, "gtk_grid_set_row_spacing");
  SP_REQ(gtk_separator_new, "gtk_separator_new");
  SP_REQ(gtk_drawing_area_new, "gtk_drawing_area_new");
  SP_REQ(gtk_drawing_area_set_content_width, "gtk_drawing_area_set_content_width");
  SP_REQ(gtk_drawing_area_set_content_height, "gtk_drawing_area_set_content_height");
  SP_REQ(gtk_drawing_area_set_draw_func, "gtk_drawing_area_set_draw_func");
  SP_REQ(gtk_event_controller_key_new, "gtk_event_controller_key_new");
  SP_REQ(gtk_event_controller_motion_new, "gtk_event_controller_motion_new");
  SP_REQ(gtk_gesture_click_new, "gtk_gesture_click_new");
  SP_REQ(gtk_widget_add_controller, "gtk_widget_add_controller");
  SP_REQ(gtk_widget_set_hexpand, "gtk_widget_set_hexpand");
  SP_REQ(gtk_widget_set_vexpand, "gtk_widget_set_vexpand");
  SP_REQ(gtk_widget_set_margin_top, "gtk_widget_set_margin_top");
  SP_REQ(gtk_widget_set_margin_bottom, "gtk_widget_set_margin_bottom");
  SP_REQ(gtk_widget_set_margin_start, "gtk_widget_set_margin_start");
  SP_REQ(gtk_widget_set_margin_end, "gtk_widget_set_margin_end");
  SP_REQ(gtk_widget_set_size_request, "gtk_widget_set_size_request");
  SP_REQ(gtk_widget_queue_draw, "gtk_widget_queue_draw");
  SP_REQ(gtk_widget_set_focusable, "gtk_widget_set_focusable");
  SP_REQ(gtk_widget_grab_focus, "gtk_widget_grab_focus");
  SP_REQ(g_main_context_iteration, "g_main_context_iteration");
  SP_REQ(g_signal_connect_data, "g_signal_connect_data");
  SP_REQ(cairo_image_surface_create_for_data, "cairo_image_surface_create_for_data");
  SP_REQ(cairo_set_source_surface, "cairo_set_source_surface");
  SP_REQ(cairo_paint, "cairo_paint");
  SP_REQ(cairo_surface_destroy, "cairo_surface_destroy");
  SP_REQ(cairo_scale, "cairo_scale");
  SP_REQ(cairo_get_source, "cairo_get_source");
  SP_REQ(cairo_pattern_set_filter, "cairo_pattern_set_filter");
  SP_REQ(cairo_save, "cairo_save");
  SP_REQ(cairo_restore, "cairo_restore");
#undef SP_REQ
  sp_ready = 1;
  return 1;
}

static int sp_display_ok(void) {
#if defined(_WIN32) || defined(__APPLE__)
  return 1;
#else
  const char *wl = getenv("WAYLAND_DISPLAY");
  const char *xd = getenv("DISPLAY");
  if (wl && wl[0]) return 1;
  if (xd && xd[0]) return 1;
  return 0;
#endif
}

static void sp_prefer_backend(void) {
  const char *forced = getenv("GDK_BACKEND");
  const char *gui = getenv("SUPER_GUI");
  if (forced && forced[0]) return;
  if (gui && !strcmp(gui, "x11")) {
    sp.gdk_set_allowed_backends("x11");
    return;
  }
  if (gui && !strcmp(gui, "wayland")) {
    sp.gdk_set_allowed_backends("wayland");
    return;
  }
#if defined(_WIN32)
  sp.gdk_set_allowed_backends("win32");
#elif defined(__APPLE__)
  sp.gdk_set_allowed_backends("macos,quartz");
#else
  /* Wayland, then X11. A session that exports both must not land on XWayland. */
  sp.gdk_set_allowed_backends("wayland,x11");
#endif
}

static void sp_detect_backend(void) {
  void *dpy = sp.gdk_display_get_default();
  const char *name = (dpy && sp.gdk_display_get_name) ? sp.gdk_display_get_name(dpy) : "";
  if (!name) name = "";
  if (strstr(name, "wayland")) sp_backend = "wayland";
  else if (strstr(name, "win") || strstr(name, "Win")) sp_backend = "gtk-win32";
  else if (strstr(name, "mac") || strstr(name, "quartz") || strstr(name, "Quartz")) sp_backend = "gtk-macos";
  else if (name[0]) sp_backend = "x11";
  else {
#if defined(_WIN32)
    sp_backend = dpy ? "gtk-win32" : "headless";
#elif defined(__APPLE__)
    sp_backend = dpy ? "gtk-macos" : "headless";
#else
    sp_backend = "headless";
#endif
  }
}

static void sp_q_push(char **q, int *n, int *r, const char *s) {
  if (*n - *r >= SP_Q) return;
  if (*n >= SP_Q) return;
  q[*n] = sp_dup(s ? s : "");
  if (q[*n]) *n += 1;
  (void)r;
}

static int sp_q_take(char **q, int *n, int *r, char *buf, int cap) {
  const char *s;
  if (!buf || cap < 1) return 0;
  buf[0] = 0;
  if (*r >= *n) return 0;
  s = q[*r] ? q[*r] : "";
  snprintf(buf, (size_t)cap, "%s", s);
  free(q[*r]);
  q[*r] = 0;
  *r += 1;
  if (*r >= *n) {
    *r = 0;
    *n = 0;
  }
  return (int)strlen(buf);
}

static void sp_push_key(const char *s) {
  if (!s || !s[0]) return;
  if (strcmp(s, "enter") == 0) {
    if (sp_saw_enter) return;
    sp_saw_enter = 1;
  }
  sp_q_push(sp_keys, &sp_kn, &sp_kr, s);
}

static void sp_push_click(const char *s) {
  if (!s || !s[0]) return;
  sp_q_push(sp_clicks, &sp_cn, &sp_cr, s);
}

static void sp_free_data(void *data, void *closure) {
  (void)closure;
  free(data);
}

static void sp_on_click(void *widget, void *data) {
  (void)widget;
  sp_push_click(data ? (const char *)data : "");
}

static void sp_on_activate(void *widget, void *data) {
  (void)widget;
  (void)data;
  sp_push_key("enter");
}

static void sp_on_row(void *box, void *row, void *data) {
  const char *id = data ? (const char *)data : "";
  int index = sp.gtk_list_box_row_get_index(row);
  int i;
  (void)box;
  for (i = 0; i < sp_nn; i++) {
    if (sp_nodes[i].id && strcmp(sp_nodes[i].id, id) == 0) sp_nodes[i].selected = index;
  }
  sp_push_click(id);
}

static int sp_on_close(void *win, void *data) {
  (void)win;
  (void)data;
  sp_quit = 1;
  return 1;
}

static int sp_on_key(void *ctl, unsigned keyval, unsigned code, unsigned state, void *data) {
  const char *name = 0;
  char one[2];
  (void)ctl;
  (void)code;
  (void)state;
  (void)data;
  if (keyval == 0xff1bu) name = "esc";
  else if (keyval == 0xff0du || keyval == 0xff8du) name = "enter";
  else if (keyval == 0xff08u) name = "backspace";
  else if (keyval == 0xff09u) name = "tab";
  else if (keyval == 0xff51u) name = "left";
  else if (keyval == 0xff52u) name = "up";
  else if (keyval == 0xff53u) name = "right";
  else if (keyval == 0xff54u) name = "down";
  else if (keyval == 0x20u) name = " ";
  else if (keyval >= 0x21u && keyval <= 0x7eu) {
    unsigned ch = keyval;
    if (sp_widgets) return 0;
    if (ch >= 'A' && ch <= 'Z') ch = ch - 'A' + 'a';
    one[0] = (char)ch;
    one[1] = 0;
    name = one;
  }
  if (!name) return 0;
  if (sp_widgets && strcmp(name, "esc") && strcmp(name, "enter") && strcmp(name, "backspace") &&
      strcmp(name, "tab") && strcmp(name, "left") && strcmp(name, "up") && strcmp(name, "right") &&
      strcmp(name, "down"))
    return 0;
  sp_push_key(name);
  return 0;
}

static void sp_on_motion(void *ctl, double x, double y, void *data) {
  int scale = sp_scale < 1 ? 1 : sp_scale;
  (void)ctl;
  (void)data;
  sp_mx = (int)(x / (double)scale);
  sp_my = (int)(y / (double)scale);
}

static void sp_on_down(void *gesture, int n, double x, double y, void *data) {
  (void)gesture;
  (void)n;
  sp_on_motion(0, x, y, data);
  sp_down = 1;
}

static void sp_on_up(void *gesture, int n, double x, double y, void *data) {
  (void)gesture;
  (void)n;
  sp_on_motion(0, x, y, data);
  sp_down = 0;
}

static void sp_draw(void *area, void *cr, int width, int height, void *user) {
  void *surface;
  void *pattern;
  (void)area;
  (void)width;
  (void)height;
  (void)user;
  if (!sp_px || sp_pw < 1 || sp_ph < 1) return;
  surface = sp.cairo_image_surface_create_for_data((unsigned char *)sp_px, 1, sp_pw, sp_ph, sp_pw * 4);
  if (!surface) return;
  sp.cairo_save(cr);
  if (sp_scale > 1) sp.cairo_scale(cr, (double)sp_scale, (double)sp_scale);
  sp.cairo_set_source_surface(cr, surface, 0, 0);
  pattern = sp.cairo_get_source(cr);
  if (pattern) sp.cairo_pattern_set_filter(pattern, 3);
  sp.cairo_paint(cr);
  sp.cairo_restore(cr);
  sp.cairo_surface_destroy(surface);
}

static void sp_connect(void *widget, const char *signal, SpCb cb, void *data) {
  sp.g_signal_connect_data(widget, signal, cb, data, data ? sp_free_data : 0, 0);
}

static void *sp_label(const char *text, const char *klass) {
  void *label = sp.gtk_label_new(text ? text : "");
  sp.gtk_label_set_wrap(label, 1);
  sp.gtk_label_set_xalign(label, 0.f);
  sp.gtk_widget_set_hexpand(label, 1);
  if (klass && klass[0]) sp.gtk_widget_add_css_class(label, klass);
  return label;
}

static void sp_node_free(SpNode *node) {
  int i;
  free(node->kind);
  free(node->id);
  free(node->text);
  free(node->value);
  free(node->parent);
  for (i = 0; i < node->item_n; i++) free(node->items[i]);
  memset(node, 0, sizeof *node);
}

static void sp_clear_tree(void) {
  int i;
  if (sp_win) sp.gtk_window_set_child(sp_win, 0);
  for (i = 0; i < sp_nn; i++) sp_node_free(&sp_nodes[i]);
  sp_nn = 0;
  sp_root = 0;
  sp_canvas = 0;
  free(sp_script);
  sp_script = 0;
}

static SpNode *sp_by_id(const char *id) {
  int i;
  if (!id || !id[0]) return 0;
  for (i = 0; i < sp_nn; i++) {
    if (sp_nodes[i].id && strcmp(sp_nodes[i].id, id) == 0) return &sp_nodes[i];
  }
  return 0;
}

static void sp_attach(SpNode *node, void *child) {
  SpNode *parent;
  if (!child) return;
  parent = sp_by_id(node->parent);
  if (!parent || !parent->box) {
    sp.gtk_box_append(sp_root, child);
    return;
  }
  if (strcmp(parent->kind, "grid") == 0) {
    int cols = parent->cols < 1 ? 1 : parent->cols;
    int i = parent->cursor++;
    sp.gtk_grid_attach(parent->box, child, i % cols, i / cols, 1, 1);
    return;
  }
  if (strcmp(parent->kind, "tabs") == 0) {
    const char *title = "Page";
    if (parent->cursor < parent->item_n && parent->items[parent->cursor]) title = parent->items[parent->cursor];
    parent->cursor++;
    sp.gtk_notebook_append_page(parent->box, child, sp.gtk_label_new(title));
    return;
  }
  sp.gtk_box_append(parent->box, child);
}

static void *sp_radio_leader(const char *group) {
  int i;
  if (!group || !group[0]) return 0;
  for (i = 0; i < sp_nn; i++) {
    SpNode *node = &sp_nodes[i];
    if (!node->widget || !node->kind || strcmp(node->kind, "radio") != 0) continue;
    if (node->item_n > 0 && node->items[0] && strcmp(node->items[0], group) == 0) return node->widget;
  }
  return 0;
}

static int sp_tabs(char *line, char **fields, int max) {
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

static void sp_margin(void *widget, int m) {
  sp.gtk_widget_set_margin_top(widget, m);
  sp.gtk_widget_set_margin_bottom(widget, m);
  sp.gtk_widget_set_margin_start(widget, m);
  sp.gtk_widget_set_margin_end(widget, m);
}

static void sp_realize(SpNode *node) {
  void *child = 0;
  const char *kind = node->kind ? node->kind : "";
  const char *text = node->text ? node->text : "";
  const char *value = node->value ? node->value : "";
  if (!strcmp(kind, "column") || !strcmp(kind, "row")) {
    void *box = sp.gtk_box_new(strcmp(kind, "row") == 0 ? 0 : 1, 8);
    sp.gtk_widget_set_hexpand(box, 1);
    node->widget = node->box = box;
    child = box;
  } else if (!strcmp(kind, "panel")) {
    void *frame = sp.gtk_frame_new(0);
    void *box = sp.gtk_box_new(1, 8);
    sp.gtk_frame_set_child(frame, box);
    sp.gtk_widget_set_hexpand(frame, 1);
    sp_margin(box, 8);
    node->widget = frame;
    node->box = box;
    child = frame;
  } else if (!strcmp(kind, "grid")) {
    void *grid = sp.gtk_grid_new();
    sp.gtk_grid_set_column_spacing(grid, 8);
    sp.gtk_grid_set_row_spacing(grid, 8);
    sp.gtk_widget_set_hexpand(grid, 1);
    node->widget = node->box = grid;
    child = grid;
  } else if (!strcmp(kind, "scroll")) {
    void *sw = sp.gtk_scrolled_window_new();
    void *box = sp.gtk_box_new(1, 8);
    sp.gtk_scrolled_window_set_child(sw, box);
    sp.gtk_scrolled_window_set_policy(sw, 2, 1);
    sp.gtk_widget_set_vexpand(sw, 1);
    sp.gtk_widget_set_hexpand(sw, 1);
    node->widget = sw;
    node->box = box;
    child = sw;
  } else if (!strcmp(kind, "tabs")) {
    void *book = sp.gtk_notebook_new();
    sp.gtk_widget_set_hexpand(book, 1);
    node->widget = node->box = book;
    child = book;
  } else if (!strcmp(kind, "label") || !strcmp(kind, "text")) {
    child = node->widget = sp_label(text, 0);
  } else if (!strcmp(kind, "titlebar")) {
    child = node->widget = sp_label(text, "title-2");
  } else if (!strcmp(kind, "badge")) {
    child = node->widget = sp_label(text, "caption");
  } else if (!strcmp(kind, "toast") || !strcmp(kind, "flash")) {
    child = node->widget = sp_label(text, "dim-label");
  } else if (!strcmp(kind, "button")) {
    void *button = sp.gtk_button_new_with_label(text[0] ? text : (node->id ? node->id : ""));
    sp.gtk_widget_set_hexpand(button, 1);
    sp_connect(button, "clicked", (SpCb)sp_on_click, sp_dup(node->id ? node->id : ""));
    child = node->widget = button;
  } else if (!strcmp(kind, "input") || !strcmp(kind, "field")) {
    void *entry = sp.gtk_entry_new();
    if (text[0]) sp.gtk_entry_set_placeholder_text(entry, text);
    if (value[0]) sp.gtk_editable_set_text(entry, value);
    sp.gtk_widget_set_hexpand(entry, 1);
    sp_connect(entry, "activate", (SpCb)sp_on_activate, 0);
    child = node->widget = entry;
  } else if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle") ||
             !strcmp(kind, "radio")) {
    void *check = sp.gtk_check_button_new_with_label(text);
    sp.gtk_check_button_set_active(check, value[0] == '1');
    if (!strcmp(kind, "radio") && node->item_n > 0) {
      void *leader = sp_radio_leader(node->items[0]);
      if (leader) sp.gtk_check_button_set_group(check, leader);
    }
    child = node->widget = check;
  } else if (!strcmp(kind, "slider")) {
    double v = (double)atoi(value);
    void *scale = sp.gtk_scale_new_with_range(0, 0, 100, 1);
    if (v < 0) v = 0;
    if (v > 100) v = 100;
    sp.gtk_range_set_value(scale, v);
    sp.gtk_widget_set_hexpand(scale, 1);
    child = node->widget = scale;
  } else if (!strcmp(kind, "progress")) {
    double v = (double)atoi(value) / 100.0;
    void *bar = sp.gtk_progress_bar_new();
    if (v < 0) v = 0;
    if (v > 1) v = 1;
    sp.gtk_progress_bar_set_fraction(bar, v);
    sp.gtk_widget_set_hexpand(bar, 1);
    child = node->widget = bar;
  } else if (!strcmp(kind, "list")) {
    void *sw = sp.gtk_scrolled_window_new();
    void *list = sp.gtk_list_box_new();
    int i;
    sp.gtk_scrolled_window_set_child(sw, list);
    sp.gtk_scrolled_window_set_policy(sw, 2, 1);
    sp.gtk_widget_set_vexpand(sw, 1);
    sp.gtk_widget_set_hexpand(sw, 1);
    sp.gtk_widget_set_size_request(sw, -1, 120);
    for (i = 0; i < node->item_n; i++) sp.gtk_list_box_append(list, sp_label(node->items[i], 0));
    sp_connect(list, "row-activated", (SpCb)sp_on_row, sp_dup(node->id ? node->id : ""));
    child = node->widget = sw;
  } else if (!strcmp(kind, "dialog")) {
    void *box = sp.gtk_box_new(1, 4);
    sp.gtk_box_append(box, sp_label(text, "heading"));
    if (value[0]) sp.gtk_box_append(box, sp_label(value, 0));
    sp.gtk_widget_add_css_class(box, "frame");
    child = node->widget = node->box = box;
  } else if (!strcmp(kind, "spacer") || !strcmp(kind, "shape")) {
    void *gap = sp.gtk_separator_new(0);
    sp.gtk_widget_set_hexpand(gap, 1);
    child = node->widget = gap;
  } else if (!strcmp(kind, "chrome")) {
    return;
  } else if (text[0]) {
    child = node->widget = sp_label(text, "dim-label");
  }
  if (child) sp_attach(node, child);
}

static void sp_finish_tabs(void) {
  int i, j;
  for (i = 0; i < sp_nn; i++) {
    SpNode *node = &sp_nodes[i];
    if (!node->kind || strcmp(node->kind, "tabs") != 0 || !node->box || node->cursor > 0) continue;
    for (j = 0; j < node->item_n; j++) {
      const char *title = node->items[j] ? node->items[j] : "";
      sp.gtk_notebook_append_page(node->box, sp_label("", 0), sp.gtk_label_new(title));
    }
  }
}

static void sp_parse(char *buf) {
  char *cursor = buf;
  while (*cursor) {
    char *line = cursor;
    char *nl = strchr(cursor, '\n');
    char *fields[8];
    int got;
    if (nl) {
      *nl = 0;
      cursor = nl + 1;
    } else {
      cursor += strlen(cursor);
    }
    if (!line[0] || !strcmp(line, "gui\t1") || !strncmp(line, "gui\t", 4)) {
      if (!strncmp(line, "gui\t", 4)) continue;
    }
    got = sp_tabs(line, fields, 8);
    if (!strcmp(fields[0], "title")) {
      if (sp_win && fields[1][0]) sp.gtk_window_set_title(sp_win, fields[1]);
      continue;
    }
    if (!strcmp(fields[0], "size")) {
      int w = atoi(fields[1]);
      int h = atoi(fields[2]);
      if (sp_win && w > 0 && h > 0) sp.gtk_window_set_default_size(sp_win, w, h);
      continue;
    }
    if (!strcmp(fields[0], "item")) {
      SpNode *node = sp_by_id(fields[1]);
      if (node && node->item_n < SP_ITEMS) node->items[node->item_n++] = sp_dup(fields[2]);
      continue;
    }
    if (!strcmp(fields[0], "node") && got >= 3 && sp_nn < SP_NODES) {
      SpNode *node = &sp_nodes[sp_nn++];
      memset(node, 0, sizeof *node);
      node->kind = sp_dup(fields[1]);
      node->id = sp_dup(fields[2]);
      node->text = sp_dup(fields[3]);
      node->value = sp_dup(fields[4]);
      node->parent = sp_dup(fields[5]);
      node->cols = atoi(fields[6]);
      if (!strcmp(node->kind ? node->kind : "", "chrome")) {
        sp_node_free(node);
        sp_nn--;
      }
    }
  }
}

static void sp_build(const char *script) {
  char *buf = sp_dup(script);
  int i;
  if (!buf) return;
  sp_clear_tree();
  sp_widgets = 1;
  sp_scale = 1;
  sp_focused = 0;
  sp_root = sp.gtk_box_new(1, 8);
  sp.gtk_widget_set_hexpand(sp_root, 1);
  sp.gtk_widget_set_vexpand(sp_root, 1);
  sp_margin(sp_root, 12);
  {
    void *sw = sp.gtk_scrolled_window_new();
    sp.gtk_scrolled_window_set_child(sw, sp_root);
    sp.gtk_scrolled_window_set_policy(sw, 2, 1);
    sp.gtk_widget_set_hexpand(sw, 1);
    sp.gtk_widget_set_vexpand(sw, 1);
    sp.gtk_window_set_child(sp_win, sw);
  }
  sp_parse(buf);
  free(buf);
  for (i = 0; i < sp_nn; i++) sp_realize(&sp_nodes[i]);
  sp_finish_tabs();
  sp_script = sp_dup(script);
  sp.gtk_window_present(sp_win);
}

static int sp_canvas_scale(int w, int h) {
  int scale = 1;
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  if (w >= 480 || h >= 480) return 1;
  while (scale < 3 && (w * scale < 480 || h * scale < 360)) scale++;
  return scale;
}

static void sp_show_canvas(int w, int h, const uint32_t *px, int pw, int ph) {
  int bw = pw > 0 ? pw : w;
  int bh = ph > 0 ? ph : h;
  int scale = sp_canvas_scale(bw, bh);
  int cw, ch;
  if (sp_widgets || !sp_canvas) {
    sp_clear_tree();
    sp_widgets = 0;
    sp_focused = 0;
    sp_canvas = sp.gtk_drawing_area_new();
    sp.gtk_drawing_area_set_draw_func(sp_canvas, sp_draw, 0, 0);
    sp.gtk_widget_set_hexpand(sp_canvas, 1);
    sp.gtk_widget_set_vexpand(sp_canvas, 1);
    sp.gtk_widget_set_focusable(sp_canvas, 1);
    {
      void *motion = sp.gtk_event_controller_motion_new();
      void *click = sp.gtk_gesture_click_new();
      sp.gtk_widget_add_controller(sp_canvas, motion);
      sp_connect(motion, "motion", (SpCb)sp_on_motion, 0);
      sp.gtk_widget_add_controller(sp_canvas, click);
      sp_connect(click, "pressed", (SpCb)sp_on_down, 0);
      sp_connect(click, "released", (SpCb)sp_on_up, 0);
    }
    sp.gtk_window_set_child(sp_win, sp_canvas);
  }
  sp_scale = scale;
  cw = bw * sp_scale;
  ch = bh * sp_scale;
  sp.gtk_drawing_area_set_content_width(sp_canvas, cw);
  sp.gtk_drawing_area_set_content_height(sp_canvas, ch);
  if (cw != sp_cw || ch != sp_ch) {
    sp.gtk_window_set_default_size(sp_win, cw, ch);
    sp_cw = cw;
    sp_ch = ch;
  }
  if (px && bw > 0 && bh > 0) {
    size_t n = (size_t)bw * (size_t)bh;
    if (bw != sp_pw || bh != sp_ph || !sp_px) {
      free(sp_px);
      sp_px = (uint32_t *)malloc(n * sizeof(uint32_t));
      sp_pw = sp_px ? bw : 0;
      sp_ph = sp_px ? bh : 0;
    }
    if (sp_px) memcpy(sp_px, px, n * sizeof(uint32_t));
    sp.gtk_widget_queue_draw(sp_canvas);
  }
  if (!sp_focused) {
    sp.gtk_widget_grab_focus(sp_canvas);
    sp_focused = 1;
  }
}

static void sp_pump(void) {
  int i;
  if (!sp.g_main_context_iteration) return;
  for (i = 0; i < 8; i++) sp.g_main_context_iteration(0, 0);
}

static void sp_arm_window(const char *title) {
  void *keys;
  void *bar;
  if (!sp_win) {
    sp_win = sp.gtk_window_new();
    sp_connect(sp_win, "close-request", (SpCb)sp_on_close, 0);
    keys = sp.gtk_event_controller_key_new();
    sp.gtk_widget_add_controller(sp_win, keys);
    sp_connect(keys, "key-pressed", (SpCb)sp_on_key, 0);
    bar = sp.gtk_header_bar_new();
    sp_heading = sp.gtk_label_new(title);
    sp.gtk_header_bar_set_title_widget(bar, sp_heading);
    sp.gtk_header_bar_set_show_title_buttons(bar, 1);
    sp.gtk_window_set_titlebar(sp_win, bar);
  }
  sp.gtk_window_set_title(sp_win, title);
  if (sp_heading) sp.gtk_label_set_text(sp_heading, title);
}

SUPER_PROVIDER_API int super_provider_open(int w, int h, const char *title) {
  if (!sp_load() || !sp_display_ok()) return 0;
  if (!title || !title[0]) title = "Super";
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  if (!sp_inited) {
    sp_prefer_backend();
    sp.gtk_init();
    sp_inited = 1;
    sp_detect_backend();
    if (!sp.gdk_display_get_default() || !strcmp(sp_backend, "headless")) {
      sp_backend = "headless";
      fprintf(stderr, "gui: GTK4 could not open a display\n");
      return 0;
    }
    fprintf(stderr, "gui: gtk on %s\n", sp_backend);
  }
  sp_arm_window(title);
  if (!sp_win) return 0;
  sp.gtk_window_set_title(sp_win, title);
  sp.gtk_window_set_default_size(sp_win, w, h);
  sp.gtk_window_present(sp_win);
  sp_quit = 0;
  sp_pump();
  return 1;
}

SUPER_PROVIDER_API void super_provider_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  if (!sp_win) return;
  if (!script) script = "";
  sp_saw_enter = 0;
  /* Five bytes: `gui`, a tab, and `1`. The sixth byte is the newline. */
  if (!strncmp(script, "gui\t1", 5)) {
    if (!sp_widgets || !sp_script || strcmp(sp_script, script) != 0) sp_build(script);
  } else {
    sp_show_canvas(w, h, px, pw, ph);
  }
  sp_pump();
}

SUPER_PROVIDER_API int super_provider_pump(void) {
  if (!sp_win) return sp_quit;
  sp_pump();
  return sp_quit;
}

SUPER_PROVIDER_API int super_provider_quit(void) { return sp_quit; }

SUPER_PROVIDER_API int super_provider_live(void) { return sp_win != 0; }

SUPER_PROVIDER_API const char *super_provider_backend(void) { return sp_backend; }

SUPER_PROVIDER_API int super_provider_take_key(char *buf, int cap) {
  return sp_q_take(sp_keys, &sp_kn, &sp_kr, buf, cap);
}

SUPER_PROVIDER_API int super_provider_take_click(char *buf, int cap) {
  return sp_q_take(sp_clicks, &sp_cn, &sp_cr, buf, cap);
}

SUPER_PROVIDER_API void super_provider_mouse(int *x, int *y, int *down) {
  if (x) *x = sp_mx;
  if (y) *y = sp_my;
  if (down) *down = sp_down;
}

SUPER_PROVIDER_API int super_provider_value(const char *id, char *buf, int cap) {
  int i;
  if (!buf || cap < 2) return 0;
  buf[0] = 0;
  if (!id) return 0;
  for (i = 0; i < sp_nn; i++) {
    SpNode *node = &sp_nodes[i];
    const char *kind;
    if (!node->id || strcmp(node->id, id) != 0 || !node->widget || !node->kind) continue;
    kind = node->kind;
    if (!strcmp(kind, "field") || !strcmp(kind, "input")) {
      const char *text = sp.gtk_editable_get_text(node->widget);
      snprintf(buf, (size_t)cap, "%s", text ? text : "");
      return (int)strlen(buf);
    }
    if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle") || !strcmp(kind, "radio")) {
      snprintf(buf, (size_t)cap, "%s", sp.gtk_check_button_get_active(node->widget) ? "1" : "0");
      return 1;
    }
    if (!strcmp(kind, "slider")) {
      int v = (int)(sp.gtk_range_get_value(node->widget) + 0.5);
      snprintf(buf, (size_t)cap, "%d", v);
      return (int)strlen(buf);
    }
    if (!strcmp(kind, "list") || !strcmp(kind, "tabs")) {
      snprintf(buf, (size_t)cap, "%d", node->selected);
      return (int)strlen(buf);
    }
  }
  return 0;
}

SUPER_PROVIDER_API void super_provider_close(void) {
  int i;
  if (sp_win && sp.gtk_window_destroy) sp.gtk_window_destroy(sp_win);
  sp_win = 0;
  sp_heading = 0;
  sp_root = 0;
  sp_canvas = 0;
  sp_widgets = 0;
  for (i = 0; i < sp_nn; i++) sp_node_free(&sp_nodes[i]);
  sp_nn = 0;
  free(sp_script);
  sp_script = 0;
  free(sp_px);
  sp_px = 0;
  sp_pw = sp_ph = 0;
  for (i = 0; i < sp_kn; i++) free(sp_keys[i]);
  for (i = 0; i < sp_cn; i++) free(sp_clicks[i]);
  sp_kn = sp_kr = sp_cn = sp_cr = 0;
  sp_quit = 0;
}
