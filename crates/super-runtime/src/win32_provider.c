/* Win32 desktop provider. Real common controls, no GTK.
 * Compiled into the runtime on every host; open() succeeds only on Windows.
 */
#ifndef SUPER_GUI_PASTED
#include "gui_provider.h"
#include "gui_doc.h"
#endif

#include <stdio.h>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <commctrl.h>

#define SW_Q 64

static GdDoc sw_doc;
static HWND sw_hwnd, sw_canvas;
static int sw_quit, sw_widgets, sw_scale, sw_mx, sw_my, sw_down;
static int sw_kn, sw_kr, sw_cn, sw_cr, sw_pw, sw_ph;
static char *sw_script;
static char *sw_keys[SW_Q];
static char *sw_clicks[SW_Q];
static uint32_t *sw_px;
static HFONT sw_font;
static int sw_ready, sw_rebuilding;
static const char *sw_backend = "win32";

static void sw_q_push(char **q, int *n, int *r, const char *s) {
  if (!s || !s[0] || *n - *r >= SW_Q || *n >= SW_Q) return;
  q[*n] = gd_dup(s);
  if (q[*n]) *n += 1;
  (void)r;
}

static int sw_q_take(char **q, int *n, int *r, char *buf, int cap) {
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

static wchar_t *sw_wide(const char *s) {
  int n;
  wchar_t *w;
  if (!s) s = "";
  n = MultiByteToWideChar(CP_UTF8, 0, s, -1, 0, 0);
  if (n < 1) n = 1;
  w = (wchar_t *)calloc((size_t)n, sizeof(wchar_t));
  if (!w) return 0;
  MultiByteToWideChar(CP_UTF8, 0, s, -1, w, n);
  return w;
}

static void sw_key(WPARAM key, int widgets) {
  const char *name = 0;
  char one[2];
  if (key == VK_ESCAPE) name = "esc";
  else if (key == VK_RETURN) name = "enter";
  else if (key == VK_BACK) name = "backspace";
  else if (key == VK_TAB) name = "tab";
  else if (key == VK_LEFT) name = "left";
  else if (key == VK_UP) name = "up";
  else if (key == VK_RIGHT) name = "right";
  else if (key == VK_DOWN) name = "down";
  else if (key == VK_SPACE) name = " ";
  else if (key >= 0x21 && key <= 0x7e) {
    unsigned ch = (unsigned)key;
    if (widgets) return;
    if (ch >= 'A' && ch <= 'Z') ch = ch - 'A' + 'a';
    one[0] = (char)ch;
    one[1] = 0;
    name = one;
  }
  if (!name) return;
  if (widgets && strcmp(name, "esc") && strcmp(name, "enter") && strcmp(name, "backspace") &&
      strcmp(name, "tab") && strcmp(name, "left") && strcmp(name, "up") && strcmp(name, "right") &&
      strcmp(name, "down"))
    return;
  sw_q_push(sw_keys, &sw_kn, &sw_kr, name);
}

static void sw_click_id(const char *id) {
  if (!sw_rebuilding) sw_q_push(sw_clicks, &sw_cn, &sw_cr, id);
}

static GdNode *sw_node(HWND hwnd) {
  return hwnd ? (GdNode *)GetWindowLongPtrW(hwnd, GWLP_USERDATA) : 0;
}

static LRESULT CALLBACK sw_canvas_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
  switch (msg) {
  case WM_PAINT: {
    PAINTSTRUCT ps;
    HDC dc = BeginPaint(hwnd, &ps);
    if (sw_px && sw_pw > 0 && sw_ph > 0) {
      BITMAPINFO info;
      int scale = sw_scale < 1 ? 1 : sw_scale;
      memset(&info, 0, sizeof info);
      info.bmiHeader.biSize = sizeof info.bmiHeader;
      info.bmiHeader.biWidth = sw_pw;
      info.bmiHeader.biHeight = -sw_ph;
      info.bmiHeader.biPlanes = 1;
      info.bmiHeader.biBitCount = 32;
      info.bmiHeader.biCompression = BI_RGB;
      StretchDIBits(dc, 0, 0, sw_pw * scale, sw_ph * scale, 0, 0, sw_pw, sw_ph, sw_px, &info,
                    DIB_RGB_COLORS, SRCCOPY);
    }
    EndPaint(hwnd, &ps);
    return 0;
  }
  case WM_MOUSEMOVE:
  case WM_LBUTTONDOWN:
  case WM_LBUTTONUP: {
    int scale = sw_scale < 1 ? 1 : sw_scale;
    sw_mx = (int)((short)LOWORD(lp)) / scale;
    sw_my = (int)((short)HIWORD(lp)) / scale;
    sw_down = (msg != WM_LBUTTONUP && (msg == WM_LBUTTONDOWN || (wp & MK_LBUTTON))) ? 1 : 0;
    if (msg == WM_LBUTTONDOWN) SetFocus(hwnd);
    return 0;
  }
  case WM_KEYDOWN:
    sw_key(wp, 0);
    return 0;
  default:
    break;
  }
  return DefWindowProcW(hwnd, msg, wp, lp);
}

static LRESULT CALLBACK sw_frame_proc(HWND hwnd, UINT msg, WPARAM wp, LPARAM lp) {
  switch (msg) {
  case WM_CLOSE:
    sw_quit = 1;
    DestroyWindow(hwnd);
    return 0;
  case WM_DESTROY:
    if (hwnd == sw_hwnd) {
      sw_hwnd = 0;
      sw_canvas = 0;
      sw_quit = 1;
    }
    return 0;
  case WM_KEYDOWN:
    sw_key(wp, sw_widgets);
    return 0;
  case WM_COMMAND: {
    GdNode *node = sw_node((HWND)lp);
    if (!sw_rebuilding && node && HIWORD(wp) == BN_CLICKED) sw_click_id(node->id);
    if (!sw_rebuilding && node && HIWORD(wp) == LBN_SELCHANGE) {
      node->selected = (int)SendMessageW((HWND)lp, LB_GETCURSEL, 0, 0);
      sw_click_id(node->id);
    }
    return 0;
  }
  case WM_NOTIFY: {
    NMHDR *hdr = (NMHDR *)lp;
    if (!sw_rebuilding && hdr->code == TCN_SELCHANGE) {
      GdNode *node = sw_node(hdr->hwndFrom);
      if (node) {
        node->selected = (int)SendMessageW(hdr->hwndFrom, TCM_GETCURSEL, 0, 0);
        sw_click_id(node->id);
      }
    }
    return 0;
  }
  default:
    break;
  }
  return DefWindowProcW(hwnd, msg, wp, lp);
}

static void sw_class(const wchar_t *name, WNDPROC proc) {
  WNDCLASSEXW cls;
  memset(&cls, 0, sizeof cls);
  cls.cbSize = sizeof cls;
  cls.lpfnWndProc = proc;
  cls.hInstance = GetModuleHandleW(0);
  cls.hCursor = LoadCursor(0, IDC_ARROW);
  cls.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
  cls.lpszClassName = name;
  RegisterClassExW(&cls);
}

static int sw_boot(void) {
  INITCOMMONCONTROLSEX icc;
  if (sw_ready) return 1;
  icc.dwSize = sizeof icc;
  icc.dwICC = ICC_STANDARD_CLASSES | ICC_BAR_CLASSES | ICC_TAB_CLASSES | ICC_LISTVIEW_CLASSES |
              ICC_PROGRESS_CLASS | ICC_WIN95_CLASSES;
  InitCommonControlsEx(&icc);
  sw_class(L"SuperGuiHost", sw_frame_proc);
  sw_class(L"SuperGuiCanvas", sw_canvas_proc);
  sw_font = (HFONT)GetStockObject(DEFAULT_GUI_FONT);
  sw_ready = 1;
  return 1;
}

static void sw_pump_some(void) {
  MSG msg;
  int i;
  for (i = 0; i < 32 && PeekMessageW(&msg, 0, 0, 0, PM_REMOVE); i++) {
    TranslateMessage(&msg);
    DispatchMessageW(&msg);
  }
}

static void sw_fit(HWND hwnd, const GdNode *node) {
  if (!hwnd || node->h <= 0 || node->w <= 0) return;
  MoveWindow(hwnd, node->x, node->y, node->w, node->h, TRUE);
  SendMessageW(hwnd, WM_SETFONT, (WPARAM)sw_font, TRUE);
}

static HWND sw_make(const wchar_t *cls, const char *text, DWORD style, const GdNode *node) {
  wchar_t *wide = sw_wide(text ? text : "");
  HWND hwnd = CreateWindowExW(0, cls, wide ? wide : L"", WS_CHILD | WS_VISIBLE | style, node->x,
                              node->y, node->w > 0 ? node->w : 10, node->h > 0 ? node->h : 10,
                              sw_hwnd, 0, GetModuleHandleW(0), 0);
  free(wide);
  if (hwnd) {
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (LONG_PTR)node);
    SendMessageW(hwnd, WM_SETFONT, (WPARAM)sw_font, TRUE);
  }
  return hwnd;
}

static void sw_clear_children(void) {
  HWND child;
  sw_rebuilding = 1;
  while ((child = GetWindow(sw_hwnd, GW_CHILD)) != 0) DestroyWindow(child);
  sw_canvas = 0;
  sw_rebuilding = 0;
}

static void sw_build(const char *script) {
  int i;
  const char *last_group = "";
  sw_clear_children();
  gd_parse(&sw_doc, script);
  if (sw_doc.title && sw_doc.title[0]) {
    wchar_t *title = sw_wide(sw_doc.title);
    if (title) SetWindowTextW(sw_hwnd, title);
    free(title);
  }
  if (sw_doc.width > 0 && sw_doc.height > 0) {
    RECT rc = {0, 0, sw_doc.width, sw_doc.height};
    AdjustWindowRect(&rc, (DWORD)GetWindowLongPtrW(sw_hwnd, GWL_STYLE), FALSE);
    SetWindowPos(sw_hwnd, 0, 0, 0, rc.right - rc.left, rc.bottom - rc.top,
                 SWP_NOMOVE | SWP_NOZORDER);
  }
  {
    RECT client;
    GetClientRect(sw_hwnd, &client);
    gd_layout_all(&sw_doc, client.right - client.left, client.bottom - client.top);
  }
  sw_widgets = 1;
  for (i = 0; i < sw_doc.n; i++) {
    GdNode *node = &sw_doc.nodes[i];
    const char *kind = node->kind ? node->kind : "";
    const char *text = node->text ? node->text : "";
    const char *value = node->value ? node->value : "";
    HWND hwnd = 0;
    if (node->h <= 0 || gd_box(kind)) continue;
    if (!strcmp(kind, "button")) {
      hwnd = sw_make(L"BUTTON", text[0] ? text : node->id, WS_TABSTOP | BS_PUSHBUTTON, node);
    } else if (!strcmp(kind, "input") || !strcmp(kind, "field")) {
      hwnd = sw_make(L"EDIT", value, WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL, node);
      if (hwnd && text[0] && !value[0]) {
        wchar_t *cue = sw_wide(text);
        if (cue) SendMessageW(hwnd, 0x1501 /* EM_SETCUEBANNER */, TRUE, (LPARAM)cue);
        free(cue);
      }
    } else if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle")) {
      hwnd = sw_make(L"BUTTON", text, WS_TABSTOP | BS_AUTOCHECKBOX, node);
      if (hwnd) SendMessageW(hwnd, BM_SETCHECK, value[0] == '1' ? BST_CHECKED : BST_UNCHECKED, 0);
    } else if (!strcmp(kind, "radio")) {
      DWORD style = WS_TABSTOP | BS_AUTORADIOBUTTON;
      const char *group = node->item_n > 0 && node->items[0] ? node->items[0] : "";
      if (!last_group[0] || strcmp(last_group, group) != 0) style |= WS_GROUP;
      last_group = group;
      hwnd = sw_make(L"BUTTON", text, style, node);
      if (hwnd) SendMessageW(hwnd, BM_SETCHECK, value[0] == '1' ? BST_CHECKED : BST_UNCHECKED, 0);
    } else if (!strcmp(kind, "slider")) {
      int v = atoi(value);
      hwnd = sw_make(TRACKBAR_CLASSW, "", TBS_HORZ | TBS_NOTICKS, node);
      if (hwnd) {
        if (v < 0) v = 0;
        if (v > 100) v = 100;
        SendMessageW(hwnd, TBM_SETRANGE, TRUE, MAKELONG(0, 100));
        SendMessageW(hwnd, TBM_SETPOS, TRUE, v);
      }
    } else if (!strcmp(kind, "progress")) {
      int v = atoi(value);
      hwnd = sw_make(PROGRESS_CLASSW, "", 0, node);
      if (hwnd) {
        if (v < 0) v = 0;
        if (v > 100) v = 100;
        SendMessageW(hwnd, PBM_SETRANGE32, 0, 100);
        SendMessageW(hwnd, PBM_SETPOS, v, 0);
      }
    } else if (!strcmp(kind, "list")) {
      int row;
      hwnd = sw_make(L"LISTBOX", "", WS_VSCROLL | WS_BORDER | LBS_NOTIFY, node);
      if (hwnd) {
        for (row = 0; row < node->item_n; row++) {
          wchar_t *item = sw_wide(node->items[row]);
          if (item) SendMessageW(hwnd, LB_ADDSTRING, 0, (LPARAM)item);
          free(item);
        }
        node->selected = atoi(value);
        SendMessageW(hwnd, LB_SETCURSEL, node->selected, 0);
      }
    } else if (!strcmp(kind, "tabs")) {
      int tab;
      hwnd = sw_make(WC_TABCONTROLW, "", WS_CLIPSIBLINGS, node);
      if (hwnd) {
        for (tab = 0; tab < node->item_n; tab++) {
          TCITEMW item;
          wchar_t *label = sw_wide(node->items[tab]);
          memset(&item, 0, sizeof item);
          item.mask = TCIF_TEXT;
          item.pszText = label ? label : L"";
          SendMessageW(hwnd, TCM_INSERTITEMW, tab, (LPARAM)&item);
          free(label);
        }
        node->selected = atoi(value);
        SendMessageW(hwnd, TCM_SETCURSEL, node->selected, 0);
      }
    } else if (!strcmp(kind, "separator") || !strcmp(kind, "spacer") || !strcmp(kind, "shape")) {
      hwnd = sw_make(L"STATIC", "", SS_ETCHEDHORZ, node);
    } else if (!strcmp(kind, "dialog")) {
      char line[512];
      snprintf(line, sizeof line, "%s%s%s", text, value[0] ? "\n" : "", value);
      hwnd = sw_make(L"STATIC", line, SS_LEFT, node);
    } else if (text[0] || value[0]) {
      hwnd = sw_make(L"STATIC", text[0] ? text : value, SS_LEFT | SS_NOPREFIX, node);
    }
    node->widget = hwnd;
    sw_fit(hwnd, node);
  }
  free(sw_script);
  sw_script = gd_dup(script);
}

static int sw_canvas_scale(int w, int h) {
  int scale = 1;
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  if (w >= 480 || h >= 480) return 1;
  while (scale < 3 && (w * scale < 480 || h * scale < 360)) scale++;
  return scale;
}

static void sw_show_canvas(int w, int h, const uint32_t *px, int pw, int ph) {
  int bw = pw > 0 ? pw : w;
  int bh = ph > 0 ? ph : h;
  int scale = sw_canvas_scale(bw, bh);
  RECT rc;
  if (sw_widgets || !sw_canvas) {
    sw_clear_children();
    sw_widgets = 0;
    sw_canvas = CreateWindowExW(0, L"SuperGuiCanvas", L"", WS_CHILD | WS_VISIBLE, 0, 0, bw * scale,
                                bh * scale, sw_hwnd, 0, GetModuleHandleW(0), 0);
  }
  sw_scale = scale;
  SetWindowPos(sw_canvas, 0, 0, 0, bw * scale, bh * scale, SWP_NOZORDER);
  rc.left = 0;
  rc.top = 0;
  rc.right = bw * scale;
  rc.bottom = bh * scale;
  AdjustWindowRect(&rc, (DWORD)GetWindowLongPtrW(sw_hwnd, GWL_STYLE), FALSE);
  SetWindowPos(sw_hwnd, 0, 0, 0, rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER);
  if (px && bw > 0 && bh > 0) {
    size_t n = (size_t)bw * (size_t)bh;
    if (bw != sw_pw || bh != sw_ph || !sw_px) {
      free(sw_px);
      sw_px = (uint32_t *)malloc(n * sizeof(uint32_t));
      sw_pw = sw_px ? bw : 0;
      sw_ph = sw_px ? bh : 0;
    }
    if (sw_px) memcpy(sw_px, px, n * sizeof(uint32_t));
    InvalidateRect(sw_canvas, 0, FALSE);
  }
  if (sw_canvas) SetFocus(sw_canvas);
}

static int sw_open(int w, int h, const char *title) {
  wchar_t *wide;
  RECT rc;
  if (!sw_boot()) return 0;
  if (!title || !title[0]) title = "Super";
  if (w < 1) w = 640;
  if (h < 1) h = 400;
  if (!sw_hwnd) {
    rc.left = 0;
    rc.top = 0;
    rc.right = w;
    rc.bottom = h;
    AdjustWindowRect(&rc, WS_OVERLAPPEDWINDOW, FALSE);
    wide = sw_wide(title);
    sw_hwnd = CreateWindowExW(0, L"SuperGuiHost", wide ? wide : L"Super", WS_OVERLAPPEDWINDOW,
                              CW_USEDEFAULT, CW_USEDEFAULT, rc.right - rc.left, rc.bottom - rc.top, 0,
                              0, GetModuleHandleW(0), 0);
    free(wide);
    if (!sw_hwnd) return 0;
  } else {
    wchar_t *again = sw_wide(title);
    if (again) SetWindowTextW(sw_hwnd, again);
    free(again);
  }
  ShowWindow(sw_hwnd, SW_SHOWNORMAL);
  UpdateWindow(sw_hwnd);
  sw_quit = 0;
  sw_pump_some();
  fprintf(stderr, "gui: win32\n");
  return 1;
}

static void sw_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  if (!sw_hwnd) return;
  if (!script) script = "";
  if (!strncmp(script, "gui\t1", 5)) {
    if (!sw_widgets || !sw_script || strcmp(sw_script, script) != 0) sw_build(script);
  } else {
    sw_show_canvas(w, h, px, pw, ph);
  }
  sw_pump_some();
}

static int sw_pump(void) {
  if (!sw_hwnd) return sw_quit;
  sw_pump_some();
  return sw_quit;
}

static int sw_is_quit(void) { return sw_quit; }
static int sw_live(void) { return sw_hwnd != 0; }
static const char *sw_name(void) { return sw_backend; }

static int sw_take_key(char *buf, int cap) { return sw_q_take(sw_keys, &sw_kn, &sw_kr, buf, cap); }
static int sw_take_click(char *buf, int cap) {
  return sw_q_take(sw_clicks, &sw_cn, &sw_cr, buf, cap);
}

static void sw_mouse(int *x, int *y, int *down) {
  if (x) *x = sw_mx;
  if (y) *y = sw_my;
  if (down) *down = sw_down;
}

static int sw_copy_text(HWND hwnd, char *buf, int cap) {
  int n;
  wchar_t wide[512];
  n = GetWindowTextW(hwnd, wide, 512);
  if (n < 0) n = 0;
  wide[n] = 0;
  return WideCharToMultiByte(CP_UTF8, 0, wide, -1, buf, cap, 0, 0) > 0 ? (int)strlen(buf) : 0;
}

static int sw_value(const char *id, char *buf, int cap) {
  GdNode *node = gd_by_id(&sw_doc, id);
  const char *kind;
  if (!buf || cap < 2) return 0;
  buf[0] = 0;
  if (!node || !node->widget || !node->kind) return 0;
  kind = node->kind;
  if (!strcmp(kind, "field") || !strcmp(kind, "input")) return sw_copy_text((HWND)node->widget, buf, cap);
  if (!strcmp(kind, "check") || !strcmp(kind, "checkbox") || !strcmp(kind, "toggle") ||
      !strcmp(kind, "radio")) {
    snprintf(buf, (size_t)cap, "%s",
             SendMessageW((HWND)node->widget, BM_GETCHECK, 0, 0) == BST_CHECKED ? "1" : "0");
    return 1;
  }
  if (!strcmp(kind, "slider")) {
    snprintf(buf, (size_t)cap, "%d", (int)SendMessageW((HWND)node->widget, TBM_GETPOS, 0, 0));
    return (int)strlen(buf);
  }
  if (!strcmp(kind, "list")) {
    node->selected = (int)SendMessageW((HWND)node->widget, LB_GETCURSEL, 0, 0);
    snprintf(buf, (size_t)cap, "%d", node->selected);
    return (int)strlen(buf);
  }
  if (!strcmp(kind, "tabs")) {
    node->selected = (int)SendMessageW((HWND)node->widget, TCM_GETCURSEL, 0, 0);
    snprintf(buf, (size_t)cap, "%d", node->selected);
    return (int)strlen(buf);
  }
  return 0;
}

static void sw_close(void) {
  int i;
  if (sw_hwnd) DestroyWindow(sw_hwnd);
  sw_hwnd = 0;
  sw_canvas = 0;
  sw_widgets = 0;
  gd_clear(&sw_doc);
  free(sw_script);
  sw_script = 0;
  free(sw_px);
  sw_px = 0;
  sw_pw = sw_ph = 0;
  for (i = 0; i < sw_kn; i++) free(sw_keys[i]);
  for (i = 0; i < sw_cn; i++) free(sw_clicks[i]);
  sw_kn = sw_kr = sw_cn = sw_cr = 0;
}

#else

static int sw_open(int w, int h, const char *title) {
  (void)w;
  (void)h;
  (void)title;
  return 0;
}
static void sw_present(int w, int h, const char *script, const uint32_t *px, int pw, int ph) {
  (void)w;
  (void)h;
  (void)script;
  (void)px;
  (void)pw;
  (void)ph;
}
static int sw_pump(void) { return 0; }
static int sw_is_quit(void) { return 0; }
static int sw_live(void) { return 0; }
static const char *sw_name(void) { return "win32"; }
static int sw_take_key(char *buf, int cap) {
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static int sw_take_click(char *buf, int cap) {
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static void sw_mouse(int *x, int *y, int *down) {
  if (x) *x = 0;
  if (y) *y = 0;
  if (down) *down = 0;
}
static int sw_value(const char *id, char *buf, int cap) {
  (void)id;
  if (buf && cap > 0) buf[0] = 0;
  return 0;
}
static void sw_close(void) {}

#endif

static const SuperGuiFns super_win32_fns = {
    "win32", sw_open,    sw_present,    sw_pump,      sw_is_quit, sw_live,
    sw_name, sw_take_key, sw_take_click, sw_mouse,     sw_value,   sw_close,
};
