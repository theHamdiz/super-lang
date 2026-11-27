//! Desktop presenter for `std.gui`.
//!
//! Widget documents (`gui\t1`) become native controls. Any other script is a
//! canvas in that window. Linux uses GTK 4 and tries Wayland before X11.
//! Windows uses Win32. macOS uses Cocoa. `SUPER_GUI=gtk`, `wayland`, `x11`,
//! `win32`, `cocoa`, or `macos` selects a provider. `SUPER_GUI=0`, `headless`,
//! `quiet`, or `ppm` stays off-screen so `super test` never grabs the display.
//! `SUPER_GUI=ppm` writes a Portable Pixmap. `SUPER_GUI=1` forces a window.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use font8x8::{BASIC_FONTS, UnicodeFonts};
use super_runtime::{HostEnv, Value};

use super::{arg_int, arg_str, cb};

const MAX_EDGE: i32 = 4096;

struct GuiState {
    w: i32,
    h: i32,
    title: String,
    script: String,
    pixels: Vec<u32>,
    quit: bool,
    mx: i64,
    my: i64,
    down: i64,
    keys: std::collections::VecDeque<String>,
}

fn state() -> &'static Mutex<GuiState> {
    static STATE: OnceLock<Mutex<GuiState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(GuiState {
            w: 0,
            h: 0,
            title: String::new(),
            script: String::new(),
            pixels: Vec::new(),
            quit: false,
            mx: 0,
            my: 0,
            down: 0,
            keys: std::collections::VecDeque::new(),
        })
    })
}

fn with_state<R>(f: impl FnOnce(&mut GuiState) -> R) -> R {
    let mut guard = state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard)
}

fn edge(v: i64) -> i32 {
    v.clamp(1, MAX_EDGE as i64) as i32
}

fn pack(r: i64, g: i64, b: i64) -> u32 {
    let c = |n: i64| n.clamp(0, 255) as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

fn put(px: &mut [u32], w: i32, h: i32, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    px[(y as usize) * (w as usize) + (x as usize)] = color;
}

fn fill(px: &mut [u32], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, color: u32) {
    for yy in y..(y + rh) {
        for xx in x..(x + rw) {
            put(px, w, h, xx, yy, color);
        }
    }
}

fn stroke(px: &mut [u32], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, color: u32) {
    if rw <= 0 || rh <= 0 {
        return;
    }
    for xx in x..(x + rw) {
        put(px, w, h, xx, y, color);
        put(px, w, h, xx, y + rh - 1, color);
    }
    for yy in y..(y + rh) {
        put(px, w, h, x, yy, color);
        put(px, w, h, x + rw - 1, yy, color);
    }
}

fn glyph_bits(ch: char) -> [u8; 8] {
    BASIC_FONTS.get(ch).unwrap_or([0; 8])
}

fn blit_text(px: &mut [u32], w: i32, h: i32, x: i32, y: i32, text: &str, color: u32, scale: i32) {
    let scale = scale.max(1);
    let advance = 8 * scale;
    for (i, ch) in text.chars().enumerate() {
        let glyph = glyph_bits(ch);
        let origin = x + (i as i32) * advance;
        for row in 0..8 {
            let bits = glyph[row];
            for col in 0..8i32 {
                // font8x8: the least significant bit is the leftmost pixel.
                if bits & (1 << col) == 0 {
                    continue;
                }
                let px0 = origin + col * scale;
                let py0 = y + (row as i32) * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        put(px, w, h, px0 + dx, py0 + dy, color);
                    }
                }
            }
        }
    }
}

/// Raster `script` into `px` (`0x00RRGGBB`, length `w * h`).
pub(super) fn raster(px: &mut [u32], w: i32, h: i32, script: &str) {
    let n = (w as usize) * (h as usize);
    if px.len() != n || w <= 0 || h <= 0 {
        return;
    }
    px.fill(pack(18, 18, 24));
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("fill ") {
            let nbs = ints(rest, 7);
            if nbs.len() == 7 {
                fill(
                    px,
                    w,
                    h,
                    nbs[0] as i32,
                    nbs[1] as i32,
                    nbs[2] as i32,
                    nbs[3] as i32,
                    pack(nbs[4], nbs[5], nbs[6]),
                );
            }
        } else if let Some(rest) = line.strip_prefix("rect ") {
            let nbs = ints(rest, 7);
            if nbs.len() == 7 {
                stroke(
                    px,
                    w,
                    h,
                    nbs[0] as i32,
                    nbs[1] as i32,
                    nbs[2] as i32,
                    nbs[3] as i32,
                    pack(nbs[4], nbs[5], nbs[6]),
                );
            }
        } else if let Some(rest) = line.strip_prefix("sprite ") {
            let mut parts = rest.split_whitespace();
            let (Some(sx), Some(sy), Some(sw), Some(sh)) = (
                parts.next().and_then(|p| p.parse::<i64>().ok()),
                parts.next().and_then(|p| p.parse::<i64>().ok()),
                parts.next().and_then(|p| p.parse::<i64>().ok()),
                parts.next().and_then(|p| p.parse::<i64>().ok()),
            ) else {
                continue;
            };
            let name = parts.next().unwrap_or("");
            let tint = 80 + i64::from(name.bytes().next().unwrap_or(0)) * 3 % 160;
            fill(
                px,
                w,
                h,
                sx as i32,
                sy as i32,
                sw as i32,
                sh as i32,
                pack(tint, 120, 160),
            );
            blit_text(px, w, h, sx as i32, sy as i32, name, pack(255, 255, 255), 1);
        } else if let Some(rest) = line.strip_prefix("text ") {
            let Some((head, msg)) = rest.split_once('|') else {
                continue;
            };
            let nbs = ints(head, 5);
            if nbs.len() == 5 {
                blit_text(
                    px,
                    w,
                    h,
                    nbs[0] as i32,
                    nbs[1] as i32,
                    msg,
                    pack(nbs[2], nbs[3], nbs[4]),
                    2,
                );
            }
        }
    }
}

fn ints(text: &str, n: usize) -> Vec<i64> {
    text.split_whitespace()
        .take(n)
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn resize(st: &mut GuiState, w: i32, h: i32) {
    let n = (w as usize) * (h as usize);
    if st.w != w || st.h != h || st.pixels.len() != n {
        st.w = w;
        st.h = h;
        st.pixels = vec![pack(18, 18, 24); n];
    }
}

fn present(w: i64, h: i64, script: &str) {
    let w = edge(w);
    let h = edge(h);
    let ppm = std::env::var("SUPER_GUI").ok();
    with_state(|st| {
        resize(st, w, h);
        st.script = script.to_string();
        raster(&mut st.pixels, w, h, script);
        if provider::live() {
            provider::present(w, h, script, &st.pixels);
            pull(st);
        }
        if ppm.as_deref() == Some("ppm") {
            let path = std::env::var("SUPER_GUI_OUT").unwrap_or_else(|_| "super-gui.ppm".into());
            let _ = write_ppm(&path, w, h, &st.pixels);
        }
    });
}

fn write_ppm(path: &str, w: i32, h: i32, px: &[u32]) -> std::io::Result<()> {
    let mut body = format!("P6\n{w} {h}\n255\n").into_bytes();
    body.reserve(px.len() * 3);
    for p in px {
        body.push((p >> 16) as u8);
        body.push((p >> 8) as u8);
        body.push(*p as u8);
    }
    fs::write(path, body)
}

fn gui_mode() -> u8 {
    match std::env::var("SUPER_GUI").ok().as_deref() {
        None | Some("") => 0,
        Some("1") | Some("native") | Some("window") | Some("gtk") | Some("wayland")
        | Some("x11") | Some("win32") | Some("cocoa") | Some("macos") => 1,
        Some("dump") => 2,
        _ => 3,
    }
}

fn wants_window() -> bool {
    match gui_mode() {
        1 => true,
        0 => {
            let display = std::env::var("DISPLAY").ok().filter(|s| !s.is_empty());
            let wayland = std::env::var("WAYLAND_DISPLAY")
                .ok()
                .filter(|s| !s.is_empty());
            display.is_some() || wayland.is_some()
        }
        _ => false,
    }
}

fn pull(st: &mut GuiState) {
    if !provider::live() {
        return;
    }
    if provider::pump() {
        st.quit = true;
    }
    let (x, y, down) = provider::mouse();
    st.mx = i64::from(x);
    st.my = i64::from(y);
    st.down = i64::from(down);
    while let Some(key) = provider::take_key() {
        st.keys.push_back(key);
    }
}

fn ensure_window() -> bool {
    if !wants_window() {
        return provider::live();
    }
    let (w, h, title) = with_state(|st| {
        if st.w <= 0 {
            st.w = 640;
        }
        if st.h <= 0 {
            st.h = 400;
        }
        (st.w, st.h, st.title.clone())
    });
    provider::open(w, h, &title)
}

mod provider {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int};

    unsafe extern "C" {
        fn super_provider_open(w: c_int, h: c_int, title: *const c_char) -> c_int;
        fn super_provider_present(
            w: c_int,
            h: c_int,
            script: *const c_char,
            px: *const u32,
            pw: c_int,
            ph: c_int,
        );
        fn super_provider_pump() -> c_int;
        fn super_provider_take_key(buf: *mut c_char, cap: c_int) -> c_int;
        fn super_provider_take_click(buf: *mut c_char, cap: c_int) -> c_int;
        fn super_provider_value(id: *const c_char, buf: *mut c_char, cap: c_int) -> c_int;
        fn super_provider_mouse(x: *mut c_int, y: *mut c_int, down: *mut c_int);
        fn super_provider_close();
        fn super_provider_backend() -> *const c_char;
        fn super_provider_live() -> c_int;
    }

    pub(super) fn live() -> bool {
        unsafe { super_provider_live() != 0 }
    }

    pub(super) fn open(w: i32, h: i32, title: &str) -> bool {
        let Ok(title) = CString::new(title) else {
            return false;
        };
        unsafe { super_provider_open(w, h, title.as_ptr()) != 0 }
    }

    pub(super) fn present(w: i32, h: i32, script: &str, pixels: &[u32]) {
        let Ok(script) = CString::new(script) else {
            return;
        };
        let (ptr, pw, ph) = if pixels.is_empty() {
            (std::ptr::null(), 0, 0)
        } else {
            (pixels.as_ptr(), w, h)
        };
        unsafe { super_provider_present(w, h, script.as_ptr(), ptr, pw, ph) }
    }

    pub(super) fn pump() -> bool {
        unsafe { super_provider_pump() != 0 }
    }

    pub(super) fn close() {
        unsafe { super_provider_close() }
    }

    pub(super) fn mouse() -> (i32, i32, i32) {
        let mut x = 0;
        let mut y = 0;
        let mut down = 0;
        unsafe { super_provider_mouse(&mut x, &mut y, &mut down) }
        (x, y, down)
    }

    fn take(mut read: impl FnMut(*mut c_char, c_int) -> c_int) -> Option<String> {
        let mut buf = [0u8; 256];
        let n = read(buf.as_mut_ptr().cast(), buf.len() as c_int);
        if n <= 0 {
            return None;
        }
        let bytes = &buf[..n as usize];
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    pub(super) fn take_key() -> Option<String> {
        take(|buf, cap| unsafe { super_provider_take_key(buf, cap) })
    }

    pub(super) fn take_click() -> String {
        take(|buf, cap| unsafe { super_provider_take_click(buf, cap) }).unwrap_or_default()
    }

    pub(super) fn value(id: &str) -> String {
        let Ok(id) = CString::new(id) else {
            return String::new();
        };
        let mut buf = [0u8; 1024];
        let n = unsafe {
            super_provider_value(id.as_ptr(), buf.as_mut_ptr().cast(), buf.len() as c_int)
        };
        if n <= 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..n as usize]).into_owned()
    }

    pub(super) fn backend() -> String {
        unsafe {
            let ptr = super_provider_backend();
            if ptr.is_null() {
                return "headless".to_string();
            }
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}

fn reg(env: &mut HostEnv, name: &str, f: impl Fn(&[Value]) -> Value + Send + Sync + 'static) {
    let cb = cb(f);
    env.register_fn(name, std::sync::Arc::clone(&cb));
    env.register_fn(format!("host.{name}"), cb);
}

pub(super) fn register(env: &mut HostEnv) {
    reg(env, "sys_gui_open", |args| {
        let w = edge(arg_int(args, 0));
        let h = edge(arg_int(args, 1));
        let title = arg_str(args, 2);
        with_state(|st| {
            resize(st, w, h);
            st.title = title;
            st.quit = false;
        });
        Value::Bool(ensure_window())
    });
    reg(env, "sys_window_open", |args| {
        let w = edge(arg_int(args, 0));
        let h = edge(arg_int(args, 1));
        let title = arg_str(args, 2);
        with_state(|st| {
            resize(st, w, h);
            st.title = title.clone();
            st.quit = false;
        });
        let live = ensure_window();
        let mut fields = HashMap::new();
        fields.insert("width".into(), Value::Int(i64::from(w)));
        fields.insert("height".into(), Value::Int(i64::from(h)));
        fields.insert("title".into(), Value::Str(title));
        fields.insert("headless".into(), Value::Bool(!live));
        Value::Struct {
            name: None,
            label: None,
            fields,
        }
    });
    reg(env, "sys_window_poll", |_| {
        let (quit, mx, my) = with_state(|st| {
            pull(st);
            (st.quit, st.mx, st.my)
        });
        let mut fields = HashMap::new();
        fields.insert("quit".into(), Value::Bool(quit));
        fields.insert("keys".into(), Value::Array(Arc::new(Vec::new())));
        fields.insert("mouse_x".into(), Value::Float(mx as f64));
        fields.insert("mouse_y".into(), Value::Float(my as f64));
        Value::Struct {
            name: None,
            label: None,
            fields,
        }
    });
    reg(env, "sys_gui_present", |args| {
        present(arg_int(args, 0), arg_int(args, 1), &arg_str(args, 2));
        Value::Bool(true)
    });
    reg(env, "sys_gui_pixel", |args| {
        let x = arg_int(args, 0);
        let y = arg_int(args, 1);
        let color = with_state(|st| {
            if x < 0 || y < 0 || x >= st.w as i64 || y >= st.h as i64 || st.pixels.is_empty() {
                return 0;
            }
            st.pixels[(y as usize) * (st.w as usize) + (x as usize)] as i64
        });
        Value::Int(color)
    });
    reg(env, "sys_gui_last", |_| {
        Value::Str(with_state(|st| st.script.clone()))
    });
    reg(env, "sys_gui_quit", |_| {
        Value::Bool(with_state(|st| {
            pull(st);
            st.quit
        }))
    });
    reg(env, "sys_gui_mouse_x", |_| {
        Value::Int(with_state(|st| {
            pull(st);
            st.mx
        }))
    });
    reg(env, "sys_gui_mouse_y", |_| {
        Value::Int(with_state(|st| {
            pull(st);
            st.my
        }))
    });
    reg(env, "sys_gui_mouse_down", |_| {
        Value::Int(with_state(|st| {
            pull(st);
            st.down
        }))
    });
    reg(env, "sys_gui_key", |_| {
        Value::Str(with_state(|st| {
            pull(st);
            st.keys.pop_front().unwrap_or_default()
        }))
    });
    reg(env, "sys_gui_click", |_| Value::Str(provider::take_click()));
    reg(env, "sys_gui_value", |args| {
        Value::Str(provider::value(&arg_str(args, 0)))
    });
    reg(env, "sys_gui_backend", |_| Value::Str(provider::backend()));
    reg(env, "sys_gui_feed_key", |args| {
        with_state(|st| st.keys.push_back(arg_str(args, 0)));
        Value::Bool(true)
    });
    reg(env, "sys_gui_feed_mouse", |args| {
        with_state(|st| {
            st.mx = arg_int(args, 0);
            st.my = arg_int(args, 1);
            st.down = arg_int(args, 2);
        });
        Value::Bool(true)
    });
    reg(env, "sys_gui_feed_quit", |args| {
        with_state(|st| st.quit = arg_int(args, 0) != 0);
        Value::Bool(true)
    });
    reg(env, "sys_gui_reset", |_| {
        provider::close();
        with_state(|st| {
            st.script.clear();
            st.pixels.clear();
            st.w = 0;
            st.h = 0;
            st.quit = false;
            st.mx = 0;
            st.my = 0;
            st.down = 0;
            st.keys.clear();
        });
        Value::Bool(true)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_paints_a_packed_red_pixel_and_text_lights_a_glyph() {
        let mut px = vec![0u32; 64 * 32];
        raster(
            &mut px,
            64,
            32,
            "fill 0 0 4 4 255 0 0\ntext 8 4 255 255 255 |A\n",
        );
        assert_eq!(px[0], 0x00FF_0000);
        assert!(
            px.iter().skip(64).any(|p| *p == 0x00FF_FFFF),
            "the 8x8 glyph for A should light at least one white pixel"
        );
    }

    #[test]
    fn a_stroke_leaves_the_interior_on_the_background() {
        let mut px = vec![0u32; 16];
        raster(&mut px, 4, 4, "rect 0 0 4 4 0 255 0\n");
        assert_eq!(px[0], 0x0000_FF00);
        assert_eq!(px[1 + 4], pack(18, 18, 24));
    }

    #[test]
    fn an_unopened_window_names_the_platform_provider() {
        if provider::live() {
            return;
        }
        let name = provider::backend();
        #[cfg(windows)]
        assert_eq!(name, "win32");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "cocoa");
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            match std::env::var("SUPER_GUI").ok().as_deref() {
                Some("win32") => assert_eq!(name, "win32"),
                Some("cocoa") | Some("macos") => assert_eq!(name, "cocoa"),
                _ => assert_eq!(name, "gtk"),
            }
        }
    }
}
