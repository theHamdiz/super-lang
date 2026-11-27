/* Shared desktop-provider table. GTK, Win32, and Cocoa fill one of these. */
#ifndef SUPER_GUI_PROVIDER_H
#define SUPER_GUI_PROVIDER_H

#include <stdint.h>

typedef struct SuperGuiFns {
  const char *name;
  int (*open)(int w, int h, const char *title);
  void (*present)(int w, int h, const char *script, const uint32_t *px, int pw, int ph);
  int (*pump)(void);
  int (*quit)(void);
  int (*live)(void);
  const char *(*backend)(void);
  int (*take_key)(char *buf, int cap);
  int (*take_click)(char *buf, int cap);
  void (*mouse)(int *x, int *y, int *down);
  int (*value)(const char *id, char *buf, int cap);
  void (*close)(void);
} SuperGuiFns;

#endif
