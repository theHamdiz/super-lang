/* Cocoa desktop provider. Real AppKit controls, no GTK.
 * Compiled into the runtime on every host; open() succeeds only on macOS.
 */
#ifndef SUPER_GUI_PASTED
#include "gui_provider.h"
#include "gui_doc.h"
#endif

#include <stdio.h>

#if defined(__APPLE__)
#include <objc/message.h>
#include <objc/runtime.h>
#include <CoreGraphics/CoreGraphics.h>

#define SC_Q 64

typedef struct ScPoint {
  double x, y;
} ScPoint;
typedef struct ScSize {
  double w, h;
} ScSize;
typedef struct ScRect {
  double x, y, w, h;
} ScRect;

static GdDoc sc_doc;
static id sc_app, sc_win, sc_canvas, sc_target;
static int sc_quit, sc_widgets, sc_scale, sc_mx, sc_my, sc_down;
static int sc_kn, sc_kr, sc_cn, sc_cr, sc_pw, sc_ph, sc_ready;
static char *sc_script;
static char *sc_keys[SC_Q];
static char *sc_clicks[SC_Q];
static uint32_t *sc_px;
static Class sc_canvas_class;
static const char *sc_backend = "cocoa";

extern id NSDefaultRunLoopMode;

static id sc_cls(const char *name) { return (id)objc_getClass(name); }
static SEL sc_sel(const char *name) { return sel_registerName(name); }

static id sc_msg(id obj, const char *sel) {
  return ((id (*)(id, SEL))objc_msgSend)(obj, sc_sel(sel));
}
static id sc_msg1(id obj, const char *sel, id a) {
  return ((id (*)(id, SEL, id))objc_msgSend)(obj, sc_sel(sel), a);
}
static void sc_void1(id obj, const char *sel, id a) {
  ((void (*)(id, SEL, id))objc_msgSend)(obj, sc_sel(sel), a);
}
static id sc_ns(const char *s) {
  return ((id (*)(id, SEL, const char *))objc_msgSend)(sc_cls("NSString"), sc_sel("stringWithUTF8String:"),
                                                      s ? s : "");
}
static long sc_long(id obj, const char *sel) {
  return ((long (*)(id, SEL))objc_msgSend)(obj, sc_sel(sel));
}
static void sc_set_frame(id view, ScRect rect) {
  ((void (*)(id, SEL, ScRect))objc_msgSend)(view, sc_sel("setFrame:"), rect);
}

#if defined(__x86_64__)
static ScPoint sc_point_msg(id obj, const char *sel) {
  ScPoint out;
  ((void (*)(ScPoint *, id, SEL))objc_msgSend_stret)(&out, obj, sc_sel(sel));
  return out;
}
static ScRect sc_rect_msg(id obj, const char *sel) {
  ScRect out;
  ((void (*)(ScRect *, id, SEL))objc_msgSend_stret)(&out, obj, sc_sel(sel));
  return out;
}
#else
static ScPoint sc_point_msg(id obj, const char *sel) {
  return ((ScPoint (*)(id, SEL))objc_msgSend)(obj, sc_sel(sel));
}
static ScRect sc_rect_msg(id obj, const char *sel) {
  return ((ScRect (*)(id, SEL))objc_msgSend)(obj, sc_sel(sel));
}
#endif

static void sc_q_push(char **q, int *n, int *r, const char *s) {
  if (!s || !s[0] || *n - *r >= SC_Q || *n >= SC_Q) return;
  q[*n] = gd_dup(s);
  if (q[*n]) *n += 1;
  (void)r;
}
static int sc_q_take(char **q, int *n, int *r, char *buf, int cap) {
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

static const char *sc_utf8(id str) {
  const char *s = ((const char *(*)(id, SEL))objc_msgSend)(str, sc_sel("UTF8String"));
  return s ? s : "";
}

static void sc_click(id self, SEL cmd, id sender) {
  id represented = sc_msg(sender, "representedObject");
  (void)self;
  (void)cmd;
  if (represented) sc_q_push(sc_clicks, &sc_cn, &sc_cr, sc_utf8(represented));
}

static void sc_will_close(id self, SEL cmd, id note) {
  (void)self;
  (void)cmd;
  (void)note;
  sc_quit = 1;
}

static signed char sc_flipped(id self, SEL cmd) {
  (void)self;
  (void)cmd;
  return 1;
}

static void sc_draw(id self, SEL cmd, ScRect dirty) {
  CGContextRef cg;
  id gfx;
  CGColorSpaceRef space;
  CGDataProviderRef provider;
  CGImageRef image;
  (void)self;
  (void)cmd;
  (void)dirty;
  if (!sc_px || sc_pw < 1 || sc_ph < 1) return;
  gfx = sc_msg(sc_cls("NSGraphicsContext"), "currentContext");
  if (!gfx) return;
  cg = (CGContextRef)((void *(*)(id, SEL))objc_msgSend)(gfx, sc_sel("CGContext"));
  if (!cg) return;
  space = CGColorSpaceCreateDeviceRGB();
  provider = CGDataProviderCreateWithData(0, sc_px, (size_t)sc_pw * (size_t)sc_ph * 4, 0);
  image = CGImageCreate((size_t)sc_pw, (size_t)sc_ph, 8, 32, (size_t)sc_pw * 4, space,
                        kCGImageAlphaNoneSkipLast | kCGBitmapByteOrder32Little, provider, 0, 0,
                        kCGRenderingIntentDefault);
  if (image) {
    CGContextDrawImage(cg, CGRectMake(0, 0, sc_pw * sc_scale, sc_ph * sc_scale), image);
    CGImageRelease(image);
  }
  CGDataProviderRelease(provider);
  CGColorSpaceRelease(space);
}

static int sc_boot(void) {
  Class target;
  Class canvas;
  if (sc_ready) return 1;
  sc_app = sc_msg(sc_cls("NSApplication"), "sharedApplication");
  ((void (*)(id, SEL, long))objc_msgSend)(sc_app, sc_sel("setActivationPolicy:"), 0);
  target = objc_allocateClassPair((Class)sc_cls("NSObject"), "SuperGuiTarget", 0);
  if (target) {
    class_addMethod(target, sc_sel("click:"), (IMP)sc_click, "v@:@");
    class_addMethod(target, sc_sel("windowWillClose:"), (IMP)sc_will_close, "v@:@");
    objc_registerClassPair(target);
  }
  sc_target = sc_msg(sc_msg((id)target, "alloc"), "init");
  canvas = objc_allocateClassPair((Class)sc_cls("NSView"), "SuperGuiCanvas", 0);
  if (canvas) {
    class_addMethod(canvas, sc_sel("isFlipped"), (IMP)sc_flipped, "c@:");
    class_addMethod(canvas, sc_sel("drawRect:"), (IMP)sc_draw, "v@:{CGRect={CGPoint=dd}{CGSize=dd}}");
    objc_registerClassPair(canvas);
    sc_canvas_class = canvas;
  }
  sc_ready = sc_app != 0;
  return sc_ready;
}

static void sc_pump_some(void) {
  int i;
  for (i = 0; i < 32; i++) {
    id ev = ((id (*)(id, SEL, unsigned long long, id, id, signed char))objc_msgSend)(
        sc_app, sc_sel("nextEventMatchingMask:untilDate:inMode:dequeue:"), ~0ULL,
        sc_msg(sc_cls("NSDate"), "distantPast"), NSDefaultRunLoopMode, 1);
    long type;
    if (!ev) break;
    sc_void1(sc_app, "sendEvent:", ev);
    type = sc_long(ev, "type");
    if (type == 10) {
      unsigned short code = ((unsigned short (*)(id, SEL))objc_msgSend)(ev, sc_sel("keyCode"));
      const char *name = 0;
      char one[8];
      if (code == 53) name = "esc";
      else if (code == 36 || code == 76) name = "enter";
      else if (code == 51) name = "backspace";
      else if (code == 48) name = "tab";
      else if (code == 123) name = "left";
      else if (code == 126) name = "up";
      else if (code == 124) name = "right";
      else if (code == 125) name = "down";
      else if (!sc_widgets) {
        id chars = sc_msg(ev, "charactersIgnoringModifiers");
        const char *utf = chars ? sc_utf8(chars) : "";
        if (utf[0] >= 32 && utf[0] < 127 && !utf[1]) {
          one[0] = utf[0] >= 'A' && utf[0] <= 'Z' ? (char)(utf[0] - 'A' + 'a') : utf[0];
          one[1] = 0;
          name = one;
        }
      }
      if (name) sc_q_push(sc_keys, &sc_kn, &sc_kr, name);
    } else if (type == 1 || type == 2 || type == 5 || type == 6) {
      id content = sc_msg(sc_win, "contentView");
      ScPoint p = ((ScPoint (*)(id, SEL, ScPoint, id))objc_msgSend)(
          content, sc_sel("convertPoint:fromView:"), sc_point_msg(ev, "locationInWindow"), 0);
      int scale = sc_scale < 1 ? 1 : sc_scale;
      sc_mx = (int)(p.x / scale);
      sc_my = (int)(p.y / scale);
      sc_down = type == 1 || type == 6;
    }
  }
}

static ScRect sc_place(const GdNode *node, double content_h) {
  ScRect rect;
  rect.x = node->x;
  rect.y = content_h - node->y - node->h;
  rect.w = node->w;
  rect.h = node->h;
  return rect;
}

static id sc_content(void) { return sc_msg(sc_win, "contentView"); }

static double sc_content_h(void) {
  ScRect frame = sc_rect_msg(sc_content(), "frame");
  return frame.h;
}

static void sc_add(id view, const GdNode *node) {
  sc_set_frame(view, sc_place(node, sc_content_h()));
  sc_void1(sc_content(), "addSubview:", view);
}

static void sc_bind(id view, const char *ident) {
  sc_void1(view, "setRepresentedObject:", sc_ns(ident ? ident : ""));
  sc_void1(view, "setTarget:", sc_target);
  sc_void1(view, "setAction:", (id)sc_sel("click:"));
}

static void sc_clear_views(void) {
  id subs = sc_msg(sc_content(), "subviews");
  while (sc_long(subs, "count") > 0) {
    id view = ((id (*)(id, SEL, unsigned long))objc_msgSend)(subs, sc_sel("objectAtIndex:"), 0);
    sc_msg(view, "removeFromSuperview");
  }
  sc_canvas = 0;
}

static id sc_button(const char *text, long type) {
  id button = sc_msg(sc_msg(sc_cls("NSButton"), "alloc"), "init");
  sc_void1(button, "setTitle:", sc_ns(text));
  ((void (*)(id, SEL, unsigned long))objc_msgSend)(button, sc_sel("setButtonType:"), (unsigned long)type);
  ((void (*)(id, SEL, unsigned long))objc_msgSend)(button, sc_sel("setBezelStyle:"), 1);
  return button;
}

static void sc_build(const char *script) {
  int i;
  id content;
  sc_clear_views();
  gd_parse(&sc_doc, script);
  if (sc_doc.title && sc_doc.title[0]) sc_void1(sc_win, "setTitle:", sc_ns(sc_doc.title));
  if (sc_doc.width > 0 && sc_doc.height > 0) {
    ScSize size = {(double)sc_doc.width, (double)sc_doc.height};
    ((void (*)(id, SEL, ScSize))objc_msgSend)(sc_win, sc_sel("setContentSize:"), size);
  }
  content = sc_content();
  {
    ScRect frame = sc_rect_msg(content, "frame");
    gd_layout_all(&sc_doc, (int)frame.w, (int)frame.h);
  }
  sc_widgets = 1;
  for (i = 0; i < sc_doc.n; i++) {
    GdNode *node = &sc_doc.nodes[i];
    const char *kind = node->kind ? node->kind : "";
    const char *text = node->text ? node->text : "";
    const char *value = node->value ? node->value : "";
    id view = 0;
    if (node->h <= 0 || gd_box(kind)) continue;
    if (!strcmp(kind, "button")) {
      view = sc_button(text[0] ? text : (node->id ? node->id : ""), 7);
      sc_bind(view, node->id);
    } else if (!strcmp(kind, "input") || !strcmp(kind, "field")) {
      view = sc_msg(sc_msg(sc_cls("NSTextField"), "alloc"), "init");
      sc_void1(view, "setStringValue:", sc_ns(value));
      if (text[0]) sc_void1(view, "setPlaceholderString:", sc_ns(text));
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setBezeled:"), 1);
    } else if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle")) {
      view = sc_button(text, 3);
      ((void (*)(id, SEL, long))objc_msgSend)(view, sc_sel("setState:"), value[0] == '1' ? 1 : 0);
    } else if (!strcmp(kind, "radio")) {
      view = sc_button(text, 4);
      ((void (*)(id, SEL, long))objc_msgSend)(view, sc_sel("setState:"), value[0] == '1' ? 1 : 0);
      sc_bind(view, node->id);
    } else if (!strcmp(kind, "slider")) {
      int v = atoi(value);
      view = sc_msg(sc_msg(sc_cls("NSSlider"), "alloc"), "init");
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setMinValue:"), 0);
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setMaxValue:"), 100);
      if (v < 0) v = 0;
      if (v > 100) v = 100;
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setDoubleValue:"), (double)v);
    } else if (!strcmp(kind, "progress")) {
      int v = atoi(value);
      view = sc_msg(sc_msg(sc_cls("NSProgressIndicator"), "alloc"), "init");
      ((void (*)(id, SEL, unsigned long))objc_msgSend)(view, sc_sel("setStyle:"), 0);
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setMinValue:"), 0);
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setMaxValue:"), 100);
      if (v < 0) v = 0;
      if (v > 100) v = 100;
      ((void (*)(id, SEL, double))objc_msgSend)(view, sc_sel("setDoubleValue:"), (double)v);
    } else if (!strcmp(kind, "list")) {
      int row;
      id popup = sc_msg(sc_msg(sc_cls("NSPopUpButton"), "alloc"), "init");
      sc_msg(popup, "removeAllItems");
      for (row = 0; row < node->item_n; row++) sc_void1(popup, "addItemWithTitle:", sc_ns(node->items[row]));
      node->selected = atoi(value);
      if (node->selected >= 0) ((void (*)(id, SEL, long))objc_msgSend)(popup, sc_sel("selectItemAtIndex:"), node->selected);
      sc_bind(popup, node->id);
      view = popup;
    } else if (!strcmp(kind, "tabs")) {
      int tab;
      view = sc_msg(sc_msg(sc_cls("NSSegmentedControl"), "alloc"), "init");
      ((void (*)(id, SEL, long))objc_msgSend)(view, sc_sel("setSegmentCount:"), node->item_n);
      for (tab = 0; tab < node->item_n; tab++) {
        ((void (*)(id, SEL, id, long))objc_msgSend)(view, sc_sel("setLabel:forSegment:"), sc_ns(node->items[tab]),
                                                   (long)tab);
      }
      node->selected = atoi(value);
      ((void (*)(id, SEL, long))objc_msgSend)(view, sc_sel("setSelectedSegment:"), node->selected);
      sc_bind(view, node->id);
    } else if (!strcmp(kind, "separator") || !strcmp(kind, "spacer") || !strcmp(kind, "shape")) {
      view = sc_msg(sc_msg(sc_cls("NSBox"), "alloc"), "init");
      ((void (*)(id, SEL, unsigned long))objc_msgSend)(view, sc_sel("setBoxType:"), 2);
    } else if (!strcmp(kind, "dialog")) {
      char line[512];
      snprintf(line, sizeof line, "%s%s%s", text, value[0] ? "\n" : "", value);
      view = sc_msg(sc_msg(sc_cls("NSTextField"), "alloc"), "init");
      sc_void1(view, "setStringValue:", sc_ns(line));
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setEditable:"), 0);
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setBezeled:"), 0);
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setDrawsBackground:"), 0);
    } else if (text[0] || value[0]) {
      view = sc_msg(sc_msg(sc_cls("NSTextField"), "alloc"), "init");
      sc_void1(view, "setStringValue:", sc_ns(text[0] ? text : value));
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setEditable:"), 0);
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setBezeled:"), 0);
      ((void (*)(id, SEL, signed char))objc_msgSend)(view, sc_sel("setDrawsBackground:"), 0);
    }
    if (view) {
      node->widget = view;
      sc_add(view, node);
    }
  }
  free(sc_script);
  sc_script = gd_dup(script);
}

static int sc_canvas_scale(int w, int h) {
  int scale = 1;
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  if (w >= 480 || h >= 480) return 1;
  while (scale < 3 && (w * scale < 480 || h * scale < 360)) scale++;
  return scale;
}

static void sc_show_canvas(int w, int h, const uint32_t *px, int pw, int ph) {
  int bw = pw > 0 ? pw : w;
  int bh = ph > 0 ? ph : h;
  int scale = sc_canvas_scale(bw, bh);
  ScSize size = {(double)(bw * scale), (double)(bh * scale)};
  if (sc_widgets || !sc_canvas) {
    sc_clear_views();
    sc_widgets = 0;
    sc_canvas = sc_msg(sc_msg((id)sc_canvas_class, "alloc"), "init");
    sc_void1(sc_content(), "addSubview:", sc_canvas);
  }
  sc_scale = scale;
  ((void (*)(id, SEL, ScSize))objc_msgSend)(sc_win, sc_sel("setContentSize:"), size);
  sc_set_frame(sc_canvas, (ScRect){0, 0, size.w, size.h});
  if (px && bw > 0 && bh > 0) {
    size_t n = (size_t)bw * (size_t)bh;
    if (bw != sc_pw || bh != sc_ph || !sc_px) {
      free(sc_px);
      sc_px = (uint32_t *)malloc(n * sizeof(uint32_t));
      sc_pw = sc_px ? bw : 0;
      sc_ph = sc_px ? bh : 0;
    }
    if (sc_px) memcpy(sc_px, px, n * sizeof(uint32_t));
    ((void (*)(id, SEL, signed char))objc_msgSend)(sc_canvas, sc_sel("setNeedsDisplay:"), 1);
  }
}

static int sc_open(int w, int h, const char *title) {
  ScRect rect;
  if (!sc_boot()) return 0;
  if (!title || !title[0]) title = "Super";
  if (w < 1) w = 640;
  if (h < 1) h = 400;
  if (!sc_win) {
    rect.x = 200;
    rect.y = 200;
    rect.w = w;
    rect.h = h;
    sc_win = ((id (*)(id, SEL, ScRect, unsigned long, unsigned long, signed char))objc_msgSend)(
        sc_msg(sc_cls("NSWindow"), "alloc"), sc_sel("initWithContentRect:styleMask:backing:defer:"), rect, 15UL,
        2UL, 0);
    if (!sc_win) return 0;
    sc_void1(sc_win, "setDelegate:", sc_target);
  }
  sc_void1(sc_win, "setTitle:", sc_ns(title));
  sc_msg(sc_win, "center");
  sc_void1(sc_win, "makeKeyAndOrderFront:", 0);
  ((void (*)(id, SEL, signed char))objc_msgSend)(sc_app, sc_sel("activateIgnoringOtherApps:"), 1);
  sc_quit = 0;
  sc_pump_some();
  fprintf(stderr, "gui: cocoa\n");
  return 1;
}

static void sc_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  if (!sc_win) return;
  if (!script) script = "";
  if (!strncmp(script, "gui\t1", 5)) {
    if (!sc_widgets || !sc_script || strcmp(sc_script, script) != 0) sc_build(script);
  } else {
    sc_show_canvas(w, h, px, pw, ph);
  }
  sc_pump_some();
}

static int sc_pump(void) {
  if (!sc_win) return sc_quit;
  if (sc_long(sc_win, "isVisible") == 0) sc_quit = 1;
  sc_pump_some();
  return sc_quit;
}
static int sc_is_quit(void) { return sc_quit; }
static int sc_live(void) { return sc_win != 0 && !sc_quit; }
static const char *sc_name(void) { return sc_backend; }
static int sc_take_key(char *buf, int cap) { return sc_q_take(sc_keys, &sc_kn, &sc_kr, buf, cap); }
static int sc_take_click(char *buf, int cap) { return sc_q_take(sc_clicks, &sc_cn, &sc_cr, buf, cap); }
static void sc_mouse(int *x, int *y, int *down) {
  if (x) *x = sc_mx;
  if (y) *y = sc_my;
  if (down) *down = sc_down;
}

static int sc_value(const char *widget_id, char *buf, int cap) {
  GdNode *node = gd_by_id(&sc_doc, widget_id);
  const char *kind;
  id view;
  if (!buf || cap < 2) return 0;
  buf[0] = 0;
  if (!node || !node->widget || !node->kind) return 0;
  kind = node->kind;
  view = (id)node->widget;
  if (!strcmp(kind, "field") || !strcmp(kind, "input") || !strcmp(kind, "dialog")) {
    snprintf(buf, (size_t)cap, "%s", sc_utf8(sc_msg(view, "stringValue")));
    return (int)strlen(buf);
  }
  if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle") ||
      !strcmp(kind, "radio")) {
    snprintf(buf, (size_t)cap, "%s", sc_long(view, "state") ? "1" : "0");
    return 1;
  }
  if (!strcmp(kind, "slider") || !strcmp(kind, "progress")) {
    double v = ((double (*)(id, SEL))objc_msgSend)(view, sc_sel("doubleValue"));
    snprintf(buf, (size_t)cap, "%d", (int)(v + 0.5));
    return (int)strlen(buf);
  }
  if (!strcmp(kind, "list")) {
    node->selected = (int)sc_long(view, "indexOfSelectedItem");
    snprintf(buf, (size_t)cap, "%d", node->selected);
    return (int)strlen(buf);
  }
  if (!strcmp(kind, "tabs")) {
    node->selected = (int)sc_long(view, "selectedSegment");
    snprintf(buf, (size_t)cap, "%d", node->selected);
    return (int)strlen(buf);
  }
  return 0;
}

static void sc_close(void) {
  int i;
  if (sc_win) sc_msg(sc_win, "close");
  sc_win = 0;
  sc_canvas = 0;
  sc_widgets = 0;
  gd_clear(&sc_doc);
  free(sc_script);
  sc_script = 0;
  free(sc_px);
  sc_px = 0;
  sc_pw = sc_ph = 0;
  for (i = 0; i < sc_kn; i++) free(sc_keys[i]);
  for (i = 0; i < sc_cn; i++) free(sc_clicks[i]);
  sc_kn = sc_kr = sc_cn = sc_cr = 0;
}

#else

static int sc_open(int w, int h, const char *title) {
  (void)w;
  (void)h;
  (void)title;
  return 0;
}
static void sc_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  (void)w;
  (void)h;
  (void)script;
  (void)px;
  (void)pw;
  (void)ph;
}
static int sc_pump(void) { return 0; }
static int sc_is_quit(void) { return 0; }
static int sc_live(void) { return 0; }
static const char *sc_name(void) { return "cocoa"; }
static int sc_take_key(char *buf, int cap) {
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static int sc_take_click(char *buf, int cap) {
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static void sc_mouse(int *x, int *y, int *down) {
  if (x) *x = 0;
  if (y) *y = 0;
  if (down) *down = 0;
}
static int sc_value(const char *id, char *buf, int cap) {
  (void)id;
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static void sc_close(void) {}

#endif

static const SuperGuiFns super_cocoa_fns = {
    "cocoa", sc_open,    sc_present,    sc_pump,      sc_is_quit, sc_live,
    sc_name, sc_take_key, sc_take_click, sc_mouse,     sc_value,   sc_close,
};
