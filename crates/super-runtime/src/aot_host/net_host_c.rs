//! AOT networking/syscall host — toolchain-owned C emission (not a standalone `.c` file).
//!
//! Round 9: HTTP/serve/routing/framing live in Super (`stdlib/net/*`, `stdlib/io/socket.sr`).
//! This module emits irreducible OS ABI wrappers into every C23 AOT binary:
//! sockets, poll/epoll, signals, stdio, req/conn arena bridges, and byte-buffer
//! helpers (`sys_bytes_*`) that the AOT `SuperVal` model needs until denser
//! Super buffers replace them.
//!
//! `net_host.c` is deleted; this Rust module is the source of truth.

/// C23 fragment appended after [`crate::c_emit`] `RUNTIME_PRELUDE`.
pub const NET_HOST_C: &str = r###"
/* Embedded into every Super C23 AOT binary: TCP, poll, signals, stdio, color. */
#if defined(__wasm__) || defined(__wasi__)
static SuperVal super_host_net(const char *hn, SuperVal *args, int nargs) {
  if (hn && strcmp(hn, "sys_req_enter") == 0) return super_int(0);
  if (hn && strcmp(hn, "sys_req_leave") == 0) return super_bool(1);
  if (hn && strcmp(hn, "sys_req_keep_str") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    return super_str(s);
  }
  if (hn && strcmp(hn, "sys_req_promote") == 0) return nargs ? args[0] : SUPER_NULL;
  if (hn && strcmp(hn, "sys_req_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(3, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int(0));
    super_obj_put(o, "promoted_objs", super_int(0));
    super_obj_put(o, "stay_objs", super_int(0));
    return super_obj_val(o);
  }
  if (hn && strcmp(hn, "sys_conn_enter") == 0) return super_int(0);
  if (hn && strcmp(hn, "sys_conn_leave") == 0) return super_int(0);
  if (hn && strcmp(hn, "sys_conn_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(4, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int(0));
    super_obj_put(o, "reclaimed_bytes", super_int(0));
    super_obj_put(o, "enter_count", super_int(0));
    super_obj_put(o, "leave_count", super_int(0));
    return super_obj_val(o);
  }
  if (hn && (strcmp(hn, "sys_stdout_write") == 0 || strcmp(hn, "sys_stderr_write") == 0)) {
    const char *s = "";
    if (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) s = args[0].as.s;
    size_t n = strlen(s);
    FILE *fp = strcmp(hn, "sys_stderr_write") == 0 ? stderr : stdout;
    if (n) {
      fwrite(s, 1, n, fp);
      fflush(fp);
    }
    return super_int((int64_t)n);
  }
  if (hn && strcmp(hn, "sys_stdio_flush") == 0) {
    fflush(stdout);
    fflush(stderr);
    return super_bool(1);
  }
  (void)args;
  (void)nargs;
  return super_err(hn && hn[0] ? hn : "unsupported on wasm");
}
#elif defined(_WIN32)
/*
 * Windows C23 AOT sockets via Winsock2 (Round 7 Option A).
 * Socket handles are returned as i64 (SOCKET cast); INVALID_SOCKET → error.
 */
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

static int g_wsa_ready = 0;

static int super_wsa_ensure(void) {
  if (g_wsa_ready) return 0;
  WSADATA wsa;
  int rc = WSAStartup(MAKEWORD(2, 2), &wsa);
  if (rc != 0) return -1;
  g_wsa_ready = 1;
  return 0;
}

static SuperVal super_sys_err_wsa(const char *op, const char *detail) {
  char buf[512];
  int e = WSAGetLastError();
  char msg[256];
  DWORD n = FormatMessageA(
      FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, NULL, (DWORD)e,
      MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT), msg, (DWORD)sizeof msg, NULL);
  if (!n) {
    snprintf(msg, sizeof msg, "WSA error %d", e);
  } else {
    /* Trim trailing CR/LF from FormatMessage. */
    while (n > 0 && (msg[n - 1] == '\r' || msg[n - 1] == '\n' || msg[n - 1] == ' ')) {
      msg[--n] = 0;
    }
  }
  if (detail && detail[0]) {
    snprintf(buf, sizeof buf, "%s %s: %s", op ? op : "syscall", detail, msg);
  } else {
    snprintf(buf, sizeof buf, "%s: %s", op ? op : "syscall", msg);
  }
  return super_err(super_strdup(buf));
}

static int super_set_nonblock_w(SOCKET fd, int on) {
  u_long mode = on ? 1u : 0u;
  return ioctlsocket(fd, FIONBIO, &mode) == 0 ? 0 : -1;
}

static int super_parse_addr_w(const char *addr, struct sockaddr_storage *ss, int *len) {
  char host[256];
  const char *colon = strrchr(addr, ':');
  if (!colon || colon == addr) return -1;
  size_t hl = (size_t)(colon - addr);
  if (hl >= sizeof host) return -1;
  memcpy(host, addr, hl);
  host[hl] = 0;
  int port = atoi(colon + 1);
  if (port < 0 || port > 65535) return -1;
  memset(ss, 0, sizeof *ss);
  if (strcmp(host, "localhost") == 0) {
    struct sockaddr_in *in = (struct sockaddr_in *)ss;
    in->sin_family = AF_INET;
    in->sin_port = htons((u_short)port);
    in->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    *len = (int)sizeof *in;
    return 0;
  }
  struct addrinfo hints, *res = NULL;
  memset(&hints, 0, sizeof hints);
  hints.ai_family = AF_UNSPEC;
  hints.ai_socktype = SOCK_STREAM;
  char portbuf[16];
  snprintf(portbuf, sizeof portbuf, "%d", port);
  if (getaddrinfo(host, portbuf, &hints, &res) != 0 || !res) return -1;
  memcpy(ss, res->ai_addr, res->ai_addrlen);
  *len = (int)res->ai_addrlen;
  freeaddrinfo(res);
  return 0;
}

static SuperVal super_fmt_sockaddr_w(const struct sockaddr *sa, socklen_t len) {
  char host[NI_MAXHOST], serv[NI_MAXSERV], out[NI_MAXHOST + NI_MAXSERV + 2];
  if (getnameinfo(sa, len, host, sizeof host, serv, sizeof serv,
                  NI_NUMERICHOST | NI_NUMERICSERV) != 0) {
    return super_err("addr");
  }
  snprintf(out, sizeof out, "%s:%s", host, serv);
  return super_str(super_strdup(out));
}

static SuperVal super_payload_str_w(SuperVal v) {
  if (v.tag == SUPER_TAG_STR && v.as.s) return v;
  if (v.tag == SUPER_TAG_ERR && v.as.s) return super_str(v.as.s);
  return super_str("");
}

static SOCKET super_as_sock(SuperVal v) {
  return (SOCKET)(uintptr_t)super_as_i(v);
}

static SuperVal super_host_net(const char *hn, SuperVal *args, int nargs) {
  if (strcmp(hn, "sys_req_enter") == 0) return super_int((int64_t)sup_req_enter());
  if (strcmp(hn, "sys_req_leave") == 0) {
    size_t mark = nargs ? (size_t)super_as_i(args[0]) : SUP_REQ_NESTED;
    sup_req_leave(mark);
    if (mark != SUP_REQ_NESTED) {
      sup_req.arena_bytes = 0;
    }
    return super_bool(1);
  }
  if (strcmp(hn, "sys_req_keep_str") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    return super_str(sup_req_keep_str(s));
  }
  if (strcmp(hn, "sys_req_promote") == 0) {
    if (!nargs) return SUPER_NULL;
    return super_req_promote(args[0]);
  }
  if (strcmp(hn, "sys_req_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(3, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int((int64_t)sup_req.arena_bytes));
    super_obj_put(o, "promoted_objs", super_int((int64_t)sup_req.promoted_objs));
    super_obj_put(o, "stay_objs", super_int((int64_t)sup_req.stay_objs));
    return super_obj_val(o);
  }
  if (strcmp(hn, "sys_conn_enter") == 0) return super_int((int64_t)sup_conn_enter());
  if (strcmp(hn, "sys_conn_leave") == 0) {
    size_t mark = nargs ? (size_t)super_as_i(args[0]) : 0;
    return super_int(sup_conn_leave(mark));
  }
  if (strcmp(hn, "sys_conn_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(4, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int((int64_t)sup_conn.arena_bytes));
    super_obj_put(o, "reclaimed_bytes", super_int((int64_t)sup_conn.reclaimed_bytes));
    super_obj_put(o, "enter_count", super_int((int64_t)sup_conn.enter_count));
    super_obj_put(o, "leave_count", super_int((int64_t)sup_conn.leave_count));
    return super_obj_val(o);
  }
  if (strcmp(hn, "sys_wants_color") == 0) return super_bool(0);
  if (strcmp(hn, "sys_isatty_fd") == 0) {
    int fd = nargs > 0 ? (int)super_as_i(args[0]) : 2;
    return super_bool(_isatty(fd));
  }
  if (strcmp(hn, "sys_stdout_write") == 0) {
    SuperVal p = nargs ? super_payload_str_w(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    if (n) { fwrite(s, 1, n, stdout); fflush(stdout); }
    return super_int((int64_t)n);
  }
  if (strcmp(hn, "sys_stderr_write") == 0) {
    SuperVal p = nargs ? super_payload_str_w(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    if (n) { fwrite(s, 1, n, stderr); fflush(stderr); }
    return super_int((int64_t)n);
  }
  if (strcmp(hn, "sys_stdio_flush") == 0) {
    fflush(stdout); fflush(stderr);
    return super_bool(1);
  }
  /* Signals are not mirrored on the Windows AOT path; keep no-op fds. */
  if (strcmp(hn, "sys_signal_arm") == 0) return super_int(-1);
  if (strcmp(hn, "sys_signal_fd") == 0) return super_int(-1);
  if (strcmp(hn, "sys_signal_signaled") == 0) return super_bool(0);
  if (strcmp(hn, "sys_signal_close") == 0) return super_bool(0);
  if (strcmp(hn, "sys_signal_inject") == 0) return super_err("signal unsupported on Windows AOT");
  if (strcmp(hn, "sys_signal_recv") == 0) return super_err("signal unsupported on Windows AOT");

  if (super_wsa_ensure() != 0) return super_err("WSAStartup failed");

  if (strcmp(hn, "sys_tcp_listen") == 0) {
    const char *addr = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    struct sockaddr_storage ss; int sl = 0;
    if (super_parse_addr_w(addr, &ss, &sl) != 0) {
      return super_err(super_strdup(
          (addr && addr[0]) ? "bad address (want host:port)" : "bad address (empty)"));
    }
    SOCKET fd = socket(ss.ss_family, SOCK_STREAM, IPPROTO_TCP);
    if (fd == INVALID_SOCKET) return super_sys_err_wsa("socket", addr);
    BOOL on = TRUE;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&on, sizeof on);
    if (bind(fd, (struct sockaddr *)&ss, sl) != 0) {
      closesocket(fd);
      return super_sys_err_wsa("bind", addr);
    }
    if (listen(fd, 128) != 0) {
      closesocket(fd);
      return super_sys_err_wsa("listen", addr);
    }
    return super_int((int64_t)(uintptr_t)fd);
  }
  if (strcmp(hn, "sys_tcp_accept") == 0) {
    SOCKET lfd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    SOCKET cfd = accept(lfd, NULL, NULL);
    if (cfd == INVALID_SOCKET) {
      int e = WSAGetLastError();
      if (e == WSAEWOULDBLOCK) return super_err("accept: would block");
      return super_sys_err_wsa("accept", NULL);
    }
    return super_int((int64_t)(uintptr_t)cfd);
  }
  if (strcmp(hn, "sys_tcp_connect") == 0) {
    const char *addr = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    struct sockaddr_storage ss; int sl = 0;
    if (super_parse_addr_w(addr, &ss, &sl) != 0) {
      return super_err(super_strdup(
          (addr && addr[0]) ? "bad address (want host:port)" : "bad address (empty)"));
    }
    SOCKET fd = socket(ss.ss_family, SOCK_STREAM, IPPROTO_TCP);
    if (fd == INVALID_SOCKET) return super_sys_err_wsa("socket", addr);
    if (connect(fd, (struct sockaddr *)&ss, sl) != 0) {
      closesocket(fd);
      return super_sys_err_wsa("connect", addr);
    }
    return super_int((int64_t)(uintptr_t)fd);
  }
  if (strcmp(hn, "sys_tcp_read") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    int64_t n = nargs > 1 ? super_as_i(args[1]) : 4096;
    if (n < 1) n = 1;
    if (n > (1 << 20)) n = 1 << 20;
    char *buf = (char *)sup_req_alloc((size_t)n + 1, 1);
    if (!buf) return super_err("oom");
    int got = recv(fd, buf, (int)n, 0);
    if (got < 0) {
      if (!sup_req_contains(buf)) free(buf);
      return super_sys_err_wsa("read", NULL);
    }
    buf[got] = 0;
    return super_str(buf);
  }
  if (strcmp(hn, "sys_tcp_write") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    SuperVal p = nargs > 1 ? super_payload_str_w(args[1]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    int w = send(fd, s, (int)n, 0);
    if (w < 0) return super_sys_err_wsa("write", NULL);
    return super_int((int64_t)w);
  }
  if (strcmp(hn, "sys_tcp_shutdown") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    int how = nargs > 1 ? (int)super_as_i(args[1]) : 2;
    int h = how == 0 ? SD_RECEIVE : how == 1 ? SD_SEND : SD_BOTH;
    if (shutdown(fd, h) != 0) return super_sys_err_wsa("shutdown", NULL);
    return super_bool(1);
  }
  if (strcmp(hn, "sys_fd_close") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    return super_bool(fd != INVALID_SOCKET && closesocket(fd) == 0);
  }
  if (strcmp(hn, "sys_socket_set_nonblock") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    int on = nargs > 1 ? (args[1].tag == SUPER_TAG_BOOL ? args[1].as.b : (int)super_as_i(args[1])) : 1;
    if (super_set_nonblock_w(fd, on) != 0) return super_sys_err_wsa("set_nonblock", NULL);
    return super_bool(1);
  }
  if (strcmp(hn, "sys_socket_set_timeout") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    int64_t rms = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t wms = nargs > 2 ? super_as_i(args[2]) : 0;
    DWORD tv;
    if (rms > 0) {
      tv = (DWORD)rms;
      setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, (const char *)&tv, sizeof tv);
    }
    if (wms > 0) {
      tv = (DWORD)wms;
      setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, (const char *)&tv, sizeof tv);
    }
    return super_bool(1);
  }
  if (strcmp(hn, "sys_fd_local_addr") == 0 || strcmp(hn, "sys_fd_peer_addr") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    struct sockaddr_storage ss; int sl = (int)sizeof ss;
    int rc = strcmp(hn, "sys_fd_local_addr") == 0
                 ? getsockname(fd, (struct sockaddr *)&ss, &sl)
                 : getpeername(fd, (struct sockaddr *)&ss, &sl);
    if (rc != 0) return super_sys_err_wsa(
        strcmp(hn, "sys_fd_local_addr") == 0 ? "local_addr" : "peer_addr", NULL);
    return super_fmt_sockaddr_w((struct sockaddr *)&ss, (socklen_t)sl);
  }
  if (strcmp(hn, "sys_net_poll2") == 0) {
    SOCKET a = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    SOCKET b = nargs > 1 ? super_as_sock(args[1]) : INVALID_SOCKET;
    int ms = nargs > 2 ? (int)super_as_i(args[2]) : -1;
    WSAPOLLFD p[2];
    memset(p, 0, sizeof p);
    p[0].fd = a; p[0].events = POLLIN;
    p[1].fd = b; p[1].events = POLLIN;
    int rc = WSAPoll(p, 2, ms);
    if (rc < 0) return super_sys_err_wsa("poll", NULL);
    int64_t bits = 0;
    if (p[0].revents & (POLLIN | POLLERR | POLLHUP)) bits |= 1;
    if (p[1].revents & (POLLIN | POLLERR | POLLHUP)) bits |= 2;
    return super_int(bits);
  }
  if (strcmp(hn, "sys_udp_bind") == 0) {
    const char *addr = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    struct sockaddr_storage ss; int sl = 0;
    if (super_parse_addr_w(addr, &ss, &sl) != 0) return super_err("bad address");
    SOCKET fd = socket(ss.ss_family, SOCK_DGRAM, IPPROTO_UDP);
    if (fd == INVALID_SOCKET) return super_sys_err_wsa("udp_socket", addr);
    BOOL on = TRUE;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, (const char *)&on, sizeof on);
    if (bind(fd, (struct sockaddr *)&ss, sl) != 0) {
      closesocket(fd);
      return super_sys_err_wsa("udp_bind", addr);
    }
    return super_int((int64_t)(uintptr_t)fd);
  }
  if (strcmp(hn, "sys_udp_send_to") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    SuperVal p = nargs > 1 ? super_payload_str_w(args[1]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    const char *addr = (nargs > 2 && args[2].tag == SUPER_TAG_STR && args[2].as.s) ? args[2].as.s : "";
    struct sockaddr_storage ss; int sl = 0;
    if (super_parse_addr_w(addr, &ss, &sl) != 0) return super_err("bad address");
    int w = sendto(fd, s, (int)strlen(s), 0, (struct sockaddr *)&ss, sl);
    if (w < 0) return super_sys_err_wsa("udp_send", addr);
    return super_int((int64_t)w);
  }
  if (strcmp(hn, "sys_udp_recv_from") == 0) {
    SOCKET fd = nargs ? super_as_sock(args[0]) : INVALID_SOCKET;
    int64_t n = nargs > 1 ? super_as_i(args[1]) : 4096;
    if (n < 1) n = 1;
    if (n > (1 << 20)) n = 1 << 20;
    char *buf = (char *)sup_req_alloc((size_t)n + 1, 1);
    if (!buf) return super_err("oom");
    struct sockaddr_storage ss; int sl = (int)sizeof ss;
    int got = recvfrom(fd, buf, (int)n, 0, (struct sockaddr *)&ss, &sl);
    if (got < 0) {
      if (!sup_req_contains(buf)) free(buf);
      return super_sys_err_wsa("udp_recv", NULL);
    }
    buf[got] = 0;
    SuperObj *o = super_obj_alloc(2, 0, NULL);
    super_obj_put(o, "data", super_str(buf));
    super_obj_put(o, "addr", super_fmt_sockaddr_w((struct sockaddr *)&ss, (socklen_t)sl));
    return super_obj_val(o);
  }
  if (strcmp(hn, "sys_bytes_concat") == 0) {
    SuperVal a = nargs > 0 ? super_payload_str_w(args[0]) : super_str("");
    SuperVal b = nargs > 1 ? super_payload_str_w(args[1]) : super_str("");
    const char *as = a.as.s ? a.as.s : "";
    const char *bs = b.as.s ? b.as.s : "";
    size_t na = strlen(as), nb = strlen(bs);
    char *o = (char *)sup_req_alloc(na + nb + 1, 1);
    if (!o) return super_err("oom");
    memcpy(o, as, na); memcpy(o + na, bs, nb); o[na + nb] = 0;
    return super_str(o);
  }
  if (strcmp(hn, "sys_bytes_to_str") == 0) return nargs ? super_payload_str_w(args[0]) : super_str("");
  if (strcmp(hn, "sys_str_to_bytes") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    size_t n = strlen(s);
    SuperArr *a = super_arr_alloc_i64(n);
    for (size_t i = 0; i < n; i++) a->i64s[i] = (unsigned char)s[i];
    return super_arr(a);
  }
  if (strcmp(hn, "sys_bytes_slice") == 0) {
    SuperVal p = nargs ? super_payload_str_w(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    int64_t lo = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t hi = nargs > 2 ? super_as_i(args[2]) : (int64_t)strlen(s);
    int64_t n = (int64_t)strlen(s);
    if (lo < 0) lo = 0;
    if (hi > n) hi = n;
    if (hi < lo) hi = lo;
    size_t len = (size_t)(hi - lo);
    char *o = (char *)sup_req_alloc(len + 1, 1);
    if (!o) return super_err("oom");
    memcpy(o, s + lo, len); o[len] = 0;
    return super_str(o);
  }
  if (strcmp(hn, "sys_bytes_find") == 0) {
    SuperVal p = nargs ? super_payload_str_w(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    const char *needle = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t from = nargs > 2 ? super_as_i(args[2]) : 0;
    if (!needle[0]) return super_int(-1);
    size_t n = strlen(s), m = strlen(needle);
    if (from < 0) from = 0;
    if ((size_t)from >= n || m > n) return super_int(-1);
    const char *hit = strstr(s + from, needle);
    return super_int(hit ? (int64_t)(hit - s) : -1);
  }
  if (strcmp(hn, "sys_epoll_create") == 0 || strcmp(hn, "sys_epoll_ctl") == 0
      || strcmp(hn, "sys_epoll_wait") == 0 || strcmp(hn, "sys_epoll_close") == 0) {
    return super_err("epoll unsupported on Windows");
  }
  return SUPER_NULL;
}
#else
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <arpa/inet.h>
#if defined(__linux__)
#include <sys/epoll.h>
#endif

static int g_sig_r = -1, g_sig_w = -1;
/* Lock-free atomic, not `volatile sig_atomic_t`: GCC 15 drops buffer stores in
   other branches of `super_host_net` when it reads a volatile here (GCC 14 and 16 do not). */
static _Atomic int g_sig_last = 0;

/* Human-readable errno errors: "bind 127.0.0.1:8080: Address already in use". */
static SuperVal super_sys_err(const char *op, const char *detail) {
  char buf[512];
  int e = errno;
  const char *msg = strerror(e);
  if (!msg || !msg[0]) msg = "unknown error";
  if (detail && detail[0]) {
    snprintf(buf, sizeof buf, "%s %s: %s", op ? op : "syscall", detail, msg);
  } else {
    snprintf(buf, sizeof buf, "%s: %s", op ? op : "syscall", msg);
  }
  return super_err(super_strdup(buf));
}

static void super_sig_handler(int signo) {
  g_sig_last = signo;
  if (g_sig_w >= 0) {
    unsigned char b = (unsigned char)signo;
    (void)write(g_sig_w, &b, 1);
  }
}

static int super_set_nonblock(int fd, int on) {
  int fl = fcntl(fd, F_GETFL, 0);
  if (fl < 0) return -1;
  if (on) fl |= O_NONBLOCK;
  else fl &= ~O_NONBLOCK;
  return fcntl(fd, F_SETFL, fl);
}

static int super_parse_addr(const char *addr, struct sockaddr_storage *ss, socklen_t *len) {
  char host[256];
  const char *colon = strrchr(addr, ':');
  if (!colon || colon == addr) return -1;
  size_t hl = (size_t)(colon - addr);
  if (hl >= sizeof host) return -1;
  memcpy(host, addr, hl);
  host[hl] = 0;
  int port = atoi(colon + 1);
  /* Port 0 is valid: ephemeral bind (getsockname reports the assigned port). */
  if (port < 0 || port > 65535) return -1;
  memset(ss, 0, sizeof *ss);
  if (strcmp(host, "localhost") == 0) {
    struct sockaddr_in *in = (struct sockaddr_in *)ss;
    in->sin_family = AF_INET;
    in->sin_port = htons((uint16_t)port);
    in->sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    *len = sizeof *in;
    return 0;
  }
  struct addrinfo hints, *res = NULL;
  memset(&hints, 0, sizeof hints);
  hints.ai_family = AF_UNSPEC;
  hints.ai_socktype = SOCK_STREAM;
  char portbuf[16];
  snprintf(portbuf, sizeof portbuf, "%d", port);
  if (getaddrinfo(host, portbuf, &hints, &res) != 0 || !res) return -1;
  memcpy(ss, res->ai_addr, res->ai_addrlen);
  *len = (socklen_t)res->ai_addrlen;
  freeaddrinfo(res);
  return 0;
}

static SuperVal super_fmt_sockaddr(const struct sockaddr *sa, socklen_t len) {
  char host[NI_MAXHOST], serv[NI_MAXSERV], out[NI_MAXHOST + NI_MAXSERV + 2];
  if (getnameinfo(sa, len, host, sizeof host, serv, sizeof serv,
                  NI_NUMERICHOST | NI_NUMERICSERV) != 0) {
    return super_err("addr");
  }
  snprintf(out, sizeof out, "%s:%s", host, serv);
  return super_str(super_strdup(out));
}

static SuperVal super_payload_str(SuperVal v) {
  if (v.tag == SUPER_TAG_STR && v.as.s) return v;
  if (v.tag == SUPER_TAG_ERR && v.as.s) return super_str(v.as.s);
  return super_str("");
}

static int super_wants_color(void) {
  const char *fc = getenv("FORCE_COLOR");
  if (fc && fc[0] && strcmp(fc, "0") != 0) return 1;
  if (getenv("NO_COLOR")) return 0;
  return isatty(STDERR_FILENO);
}

static SuperVal super_host_net(const char *hn, SuperVal *args, int nargs) {
  if (strcmp(hn, "sys_req_enter") == 0) return super_int((int64_t)sup_req_enter());
  if (strcmp(hn, "sys_req_leave") == 0) {
    size_t mark = nargs ? (size_t)super_as_i(args[0]) : SUP_REQ_NESTED;
    sup_req_leave(mark);
    if (mark != SUP_REQ_NESTED) {
      /* Per-request diagnostics reset after rewind (alloc_probe / soak). */
      sup_req.arena_bytes = 0;
      /* promoted_objs / stay_objs accumulate across leave for the process probe;
         leave only clears the bump. Counters are zeroed on the next top-level enter
         when cur returns to the same mark baseline — see escape_region docs. */
    }
    return super_bool(1);
  }
  if (strcmp(hn, "sys_req_keep_str") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    return super_str(sup_req_keep_str(s));
  }
  if (strcmp(hn, "sys_req_promote") == 0) {
    if (!nargs) return SUPER_NULL;
    return super_req_promote(args[0]);
  }
  if (strcmp(hn, "sys_req_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(3, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int((int64_t)sup_req.arena_bytes));
    super_obj_put(o, "promoted_objs", super_int((int64_t)sup_req.promoted_objs));
    super_obj_put(o, "stay_objs", super_int((int64_t)sup_req.stay_objs));
    return super_obj_val(o);
  }
  if (strcmp(hn, "sys_conn_enter") == 0) return super_int((int64_t)sup_conn_enter());
  if (strcmp(hn, "sys_conn_leave") == 0) {
    size_t mark = nargs ? (size_t)super_as_i(args[0]) : 0;
    return super_int(sup_conn_leave(mark));
  }
  if (strcmp(hn, "sys_conn_stats") == 0) {
    SuperObj *o = super_obj_alloc_durable(4, 1, NULL);
    super_obj_put(o, "arena_bytes", super_int((int64_t)sup_conn.arena_bytes));
    super_obj_put(o, "reclaimed_bytes", super_int((int64_t)sup_conn.reclaimed_bytes));
    super_obj_put(o, "enter_count", super_int((int64_t)sup_conn.enter_count));
    super_obj_put(o, "leave_count", super_int((int64_t)sup_conn.leave_count));
    return super_obj_val(o);
  }
  if (strcmp(hn, "sys_wants_color") == 0) return super_bool(super_wants_color());
  if (strcmp(hn, "sys_isatty_fd") == 0) {
    int fd = nargs > 0 ? (int)super_as_i(args[0]) : STDERR_FILENO;
    return super_bool(isatty(fd));
  }
  if (strcmp(hn, "sys_stdout_write") == 0) {
    SuperVal p = nargs ? super_payload_str(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    if (n) { fwrite(s, 1, n, stdout); fflush(stdout); }
    return super_int((int64_t)n);
  }
  if (strcmp(hn, "sys_stderr_write") == 0) {
    SuperVal p = nargs ? super_payload_str(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    if (n) { fwrite(s, 1, n, stderr); fflush(stderr); }
    return super_int((int64_t)n);
  }
  if (strcmp(hn, "sys_stdio_flush") == 0) {
    fflush(stdout); fflush(stderr);
    return super_bool(1);
  }
  if (strcmp(hn, "sys_signal_arm") == 0) {
    if (g_sig_r >= 0) { close(g_sig_r); close(g_sig_w); g_sig_r = g_sig_w = -1; }
    int fds[2];
    if (pipe(fds) != 0) return super_err("pipe");
    g_sig_r = fds[0]; g_sig_w = fds[1];
    super_set_nonblock(g_sig_r, 1);
    super_set_nonblock(g_sig_w, 1);
    g_sig_last = 0;
    int count = 0;
    if (nargs > 0 && args[0].tag == SUPER_TAG_ARR && args[0].as.arr) {
      SuperArr *a = args[0].as.arr;
      for (size_t i = 0; i < a->len; i++) {
        int64_t s = 0;
        if (a->kind == 2 && a->i64s) s = a->i64s[i];
        else if (a->kind == 0 && a->items) s = super_as_i(a->items[i]);
        if (s > 0) {
          signal((int)s, super_sig_handler);
          count++;
        }
      }
    }
    if (count == 0) {
      signal(SIGINT, super_sig_handler);
      signal(SIGTERM, super_sig_handler);
    }
    return super_int(g_sig_r);
  }
  if (strcmp(hn, "sys_signal_fd") == 0) return super_int(g_sig_r);
  if (strcmp(hn, "sys_signal_signaled") == 0) return super_bool(g_sig_last != 0);
  if (strcmp(hn, "sys_signal_close") == 0) {
    if (g_sig_r < 0) return super_bool(0);
    close(g_sig_r); close(g_sig_w);
    g_sig_r = g_sig_w = -1;
    g_sig_last = 0;
    return super_bool(1);
  }
  if (strcmp(hn, "sys_signal_inject") == 0) {
    int s = nargs ? (int)super_as_i(args[0]) : SIGTERM;
    if (g_sig_w < 0) return super_err("signal not armed");
    g_sig_last = s;
    unsigned char b = (unsigned char)s;
    if (write(g_sig_w, &b, 1) < 0) return super_err("inject");
    return super_bool(1);
  }
  if (strcmp(hn, "sys_signal_recv") == 0) {
    if (g_sig_r < 0) return super_err("signal not armed");
    super_set_nonblock(g_sig_r, 0);
    unsigned char b = 0;
    ssize_t n = read(g_sig_r, &b, 1);
    super_set_nonblock(g_sig_r, 1);
    if (n <= 0) return super_err("signal pipe");
    int last = g_sig_last;
    int s = last ? last : (int)b;
    return super_int(s);
  }
  if (strcmp(hn, "sys_tcp_listen") == 0) {
    const char *addr = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    struct sockaddr_storage ss; socklen_t sl = 0;
    if (super_parse_addr(addr, &ss, &sl) != 0) {
      return super_err(super_strdup(
          (addr && addr[0]) ? "bad address (want host:port)" : "bad address (empty)"));
    }
    int fd = socket(ss.ss_family, SOCK_STREAM, 0);
    if (fd < 0) return super_sys_err("socket", addr);
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &on, sizeof on);
    if (bind(fd, (struct sockaddr *)&ss, sl) != 0) {
      close(fd);
      return super_sys_err("bind", addr);
    }
    if (listen(fd, 128) != 0) {
      close(fd);
      return super_sys_err("listen", addr);
    }
    return super_int(fd);
  }
  if (strcmp(hn, "sys_tcp_accept") == 0) {
    int lfd = nargs ? (int)super_as_i(args[0]) : -1;
    int cfd = accept(lfd, NULL, NULL);
    if (cfd < 0) {
      if (errno == EAGAIN || errno == EWOULDBLOCK) return super_err("accept: would block");
      return super_sys_err("accept", NULL);
    }
    return super_int(cfd);
  }
  if (strcmp(hn, "sys_tcp_connect") == 0) {
    const char *addr = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    struct sockaddr_storage ss; socklen_t sl = 0;
    if (super_parse_addr(addr, &ss, &sl) != 0) {
      return super_err(super_strdup(
          (addr && addr[0]) ? "bad address (want host:port)" : "bad address (empty)"));
    }
    int fd = socket(ss.ss_family, SOCK_STREAM, 0);
    if (fd < 0) return super_sys_err("socket", addr);
    if (connect(fd, (struct sockaddr *)&ss, sl) != 0) {
      close(fd);
      return super_sys_err("connect", addr);
    }
    return super_int(fd);
  }
  if (strcmp(hn, "sys_tcp_read") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    int64_t n = nargs > 1 ? super_as_i(args[1]) : 4096;
    if (n < 1) n = 1;
    if (n > (1 << 20)) n = 1 << 20;
    char *buf = (char *)sup_req_alloc((size_t)n + 1, 1);
    if (!buf) return super_err("oom");
    ssize_t got = read(fd, buf, (size_t)n);
    if (got < 0) { if (!sup_req_contains(buf)) free(buf); return super_sys_err("read", NULL); }
    buf[got] = 0;
    return super_str(buf);
  }
  if (strcmp(hn, "sys_tcp_write") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    SuperVal p = nargs > 1 ? super_payload_str(args[1]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    size_t n = strlen(s);
    ssize_t w = write(fd, s, n);
    if (w < 0) return super_sys_err("write", NULL);
    return super_int((int64_t)w);
  }
  if (strcmp(hn, "sys_tcp_shutdown") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    int how = nargs > 1 ? (int)super_as_i(args[1]) : 2;
    int h = how == 0 ? SHUT_RD : how == 1 ? SHUT_WR : SHUT_RDWR;
    if (shutdown(fd, h) != 0) return super_sys_err("shutdown", NULL);
    return super_bool(1);
  }
  if (strcmp(hn, "sys_fd_close") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    return super_bool(fd >= 0 && close(fd) == 0);
  }
  if (strcmp(hn, "sys_socket_set_nonblock") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    int on = nargs > 1 ? (args[1].tag == SUPER_TAG_BOOL ? args[1].as.b : (int)super_as_i(args[1])) : 1;
    if (super_set_nonblock(fd, on) != 0) return super_sys_err("set_nonblock", NULL);
    return super_bool(1);
  }
  if (strcmp(hn, "sys_socket_set_timeout") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    int64_t rms = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t wms = nargs > 2 ? super_as_i(args[2]) : 0;
    struct timeval tv;
    if (rms > 0) {
      tv.tv_sec = (time_t)(rms / 1000);
      tv.tv_usec = (suseconds_t)((rms % 1000) * 1000);
      setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof tv);
    }
    if (wms > 0) {
      tv.tv_sec = (time_t)(wms / 1000);
      tv.tv_usec = (suseconds_t)((wms % 1000) * 1000);
      setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof tv);
    }
    return super_bool(1);
  }
  if (strcmp(hn, "sys_fd_local_addr") == 0 || strcmp(hn, "sys_fd_peer_addr") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    struct sockaddr_storage ss; socklen_t sl = sizeof ss;
    int rc = strcmp(hn, "sys_fd_local_addr") == 0
                 ? getsockname(fd, (struct sockaddr *)&ss, &sl)
                 : getpeername(fd, (struct sockaddr *)&ss, &sl);
    if (rc != 0) return super_sys_err(
        strcmp(hn, "sys_fd_local_addr") == 0 ? "local_addr" : "peer_addr", NULL);
    return super_fmt_sockaddr((struct sockaddr *)&ss, sl);
  }
  if (strcmp(hn, "sys_net_poll2") == 0) {
    int a = nargs ? (int)super_as_i(args[0]) : -1;
    int b = nargs > 1 ? (int)super_as_i(args[1]) : -1;
    int ms = nargs > 2 ? (int)super_as_i(args[2]) : -1;
    struct pollfd p[2] = {
      { .fd = a, .events = POLLIN, .revents = 0 },
      { .fd = b, .events = POLLIN, .revents = 0 },
    };
    int rc = poll(p, 2, ms);
    if (rc < 0) return super_sys_err("poll", NULL);
    int64_t bits = 0;
    if (p[0].revents & (POLLIN | POLLERR | POLLHUP)) bits |= 1;
    if (p[1].revents & (POLLIN | POLLERR | POLLHUP)) bits |= 2;
    return super_int(bits);
  }
  if (strcmp(hn, "sys_bytes_concat") == 0) {
    SuperVal a = nargs > 0 ? super_payload_str(args[0]) : super_str("");
    SuperVal b = nargs > 1 ? super_payload_str(args[1]) : super_str("");
    const char *as = a.as.s ? a.as.s : "";
    const char *bs = b.as.s ? b.as.s : "";
    size_t na = strlen(as), nb = strlen(bs);
    char *o = (char *)sup_req_alloc(na + nb + 1, 1);
    if (!o) return super_err("oom");
    memcpy(o, as, na); memcpy(o + na, bs, nb); o[na + nb] = 0;
    return super_str(o);
  }
  if (strcmp(hn, "sys_bytes_to_str") == 0) return nargs ? super_payload_str(args[0]) : super_str("");
  if (strcmp(hn, "sys_str_to_bytes") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    size_t n = strlen(s);
    SuperArr *a = super_arr_alloc_i64(n);
    for (size_t i = 0; i < n; i++) a->i64s[i] = (unsigned char)s[i];
    return super_arr(a);
  }
  if (strcmp(hn, "sys_bytes_slice") == 0) {
    SuperVal p = nargs ? super_payload_str(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    int64_t lo = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t hi = nargs > 2 ? super_as_i(args[2]) : (int64_t)strlen(s);
    int64_t n = (int64_t)strlen(s);
    if (lo < 0) lo = 0;
    if (hi > n) hi = n;
    if (hi < lo) hi = lo;
    size_t len = (size_t)(hi - lo);
    char *o = (char *)sup_req_alloc(len + 1, 1);
    if (!o) return super_err("oom");
    memcpy(o, s + lo, len); o[len] = 0;
    return super_str(o);
  }
  if (strcmp(hn, "sys_bytes_find") == 0) {
    SuperVal p = nargs ? super_payload_str(args[0]) : super_str("");
    const char *s = p.as.s ? p.as.s : "";
    const char *needle = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t from = nargs > 2 ? super_as_i(args[2]) : 0;
    if (!needle[0]) return super_int(-1);
    size_t n = strlen(s), m = strlen(needle);
    if (from < 0) from = 0;
    if ((size_t)from >= n || m > n) return super_int(-1);
    const char *hit = strstr(s + from, needle);
    return super_int(hit ? (int64_t)(hit - s) : -1);
  }
  if (strcmp(hn, "sys_epoll_create") == 0) {
#if defined(__linux__)
    int fd = epoll_create1(EPOLL_CLOEXEC);
    if (fd < 0) return super_err("epoll");
    return super_int(fd);
#else
    return super_err("epoll unsupported");
#endif
  }
  if (strcmp(hn, "sys_epoll_close") == 0) {
    int fd = nargs ? (int)super_as_i(args[0]) : -1;
    return super_bool(fd >= 0 && close(fd) == 0);
  }
  if (strcmp(hn, "sys_epoll_ctl") == 0) {
#if defined(__linux__)
    int ep = nargs ? (int)super_as_i(args[0]) : -1;
    int op = nargs > 1 ? (int)super_as_i(args[1]) : 0;
    int sock = nargs > 2 ? (int)super_as_i(args[2]) : -1;
    uint32_t events = nargs > 3 ? (uint32_t)super_as_i(args[3]) : 0;
    uint64_t token = nargs > 4 ? (uint64_t)super_as_i(args[4]) : 0;
    struct epoll_event ev;
    memset(&ev, 0, sizeof ev);
    ev.events = events;
    ev.data.u64 = token;
    if (epoll_ctl(ep, op, sock, op == EPOLL_CTL_DEL ? NULL : &ev) != 0)
      return super_err("epoll_ctl");
    return super_bool(1);
#else
    return super_err("epoll unsupported");
#endif
  }
  if (strcmp(hn, "sys_epoll_wait") == 0) {
#if defined(__linux__)
    int ep = nargs ? (int)super_as_i(args[0]) : -1;
    int max = nargs > 1 ? (int)super_as_i(args[1]) : 16;
    int ms = nargs > 2 ? (int)super_as_i(args[2]) : -1;
    if (max < 1) max = 1;
    if (max > 4096) max = 4096;
    struct epoll_event *evs = (struct epoll_event *)calloc((size_t)max, sizeof *evs);
    if (!evs) return super_err("oom");
    int n = epoll_wait(ep, evs, max, ms);
    if (n < 0) { free(evs); return super_err("epoll_wait"); }
    SuperArr *a = super_arr_alloc((size_t)n);
    for (int i = 0; i < n; i++) {
      SuperObj *o = super_obj_alloc(2, 0, NULL);
      super_obj_put(o, "token", super_int((int64_t)evs[i].data.u64));
      super_obj_put(o, "events", super_int((int64_t)evs[i].events));
      super_arr_push(a, super_obj_val(o));
    }
    free(evs);
    return super_arr(a);
#else
    return super_err("epoll unsupported");
#endif
  }
  return SUPER_NULL; /* not handled */
}
#endif /* wasm / Windows / POSIX */

"###;
