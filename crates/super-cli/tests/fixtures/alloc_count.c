/* LD_PRELOAD allocation counter for the server allocation probe.
 * Counts malloc/calloc/realloc/free calls and, at exit, writes
 * "allocs frees bytes" to the file named by ALLOC_COUNT_OUT. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static atomic_ulong allocs, frees, bytes;
static void *(*real_malloc)(size_t);
static void *(*real_calloc)(size_t, size_t);
static void *(*real_realloc)(void *, size_t);
static void (*real_free)(void *);

/* dlsym may call calloc before real_calloc is known. */
static _Alignas(16) char boot[1 << 16];
static size_t boot_used;

static int from_boot(const void *p) {
    return (const char *)p >= boot && (const char *)p < boot + sizeof boot;
}

void *malloc(size_t n) {
    if (!real_malloc) real_malloc = dlsym(RTLD_NEXT, "malloc");
    atomic_fetch_add(&allocs, 1);
    atomic_fetch_add(&bytes, n);
    return real_malloc(n);
}

void *calloc(size_t a, size_t b) {
    if (!real_calloc) {
        static int resolving;
        if (resolving) {
            void *p = boot + boot_used;
            boot_used += (a * b + 15) & ~(size_t)15;
            return p;
        }
        resolving = 1;
        real_calloc = dlsym(RTLD_NEXT, "calloc");
        resolving = 0;
    }
    atomic_fetch_add(&allocs, 1);
    atomic_fetch_add(&bytes, a * b);
    return real_calloc(a, b);
}

void *realloc(void *p, size_t n) {
    if (!real_realloc) real_realloc = dlsym(RTLD_NEXT, "realloc");
    atomic_fetch_add(&allocs, 1);
    atomic_fetch_add(&bytes, n);
    return real_realloc(p, n);
}

void free(void *p) {
    if (!p || from_boot(p)) return;
    if (!real_free) real_free = dlsym(RTLD_NEXT, "free");
    atomic_fetch_add(&frees, 1);
    real_free(p);
}

__attribute__((destructor)) static void report(void) {
    const char *out = getenv("ALLOC_COUNT_OUT");
    if (!out) return;
    FILE *f = fopen(out, "w");
    if (!f) return;
    fprintf(f, "%lu %lu %lu\n", (unsigned long)allocs, (unsigned long)frees, (unsigned long)bytes);
    fclose(f);
}
