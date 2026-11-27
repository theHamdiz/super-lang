/**
 * Super C ABI — drop-in embedding for C17 / C++ / Rust hosts.
 *
 * Link against libsuper_ffi (staticlib or cdylib) produced by the `super-ffi` crate.
 */
#ifndef SUPER_H
#define SUPER_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stddef.h>
#include <stdint.h>

typedef struct SuperEngine SuperEngine;

/** Native trampoline: packed C-ABI args in, optional return out. */
typedef void (*SuperHostFn)(void *user_data, void *args_buf, void *ret_buf);

/** Create a new engine. Returns NULL on failure. */
SuperEngine *super_engine_new(void);

/** Destroy an engine. */
void super_engine_free(SuperEngine *engine);

/** Compile and load a .sr module. Returns 0 on success, -1 on error. */
int super_load_file(SuperEngine *engine, const char *path);

/** Hot-reload a module (keeps host binds / stacks). Returns 0 on success. */
int super_reload_module(SuperEngine *engine, const char *path);

/** Last error message (empty if none). Valid until the next failing call. */
const char *super_last_error(SuperEngine *engine);

/**
 * Register a host function visible to Super as `module.name` (and bare `name`).
 * `sig_descriptor` examples: "f32,f32->void", "i64->i64", "Entity:16,f32->f32".
 */
int super_bind_fn(SuperEngine *engine, const char *module, const char *name,
                SuperHostFn fn_ptr, void *user_data, const char *sig_descriptor);

/**
 * Zero-copy share of a host array. Super can `for @name` over the elements.
 * Prefer super_bind_slice_ex when field layout is known.
 */
int super_bind_slice(SuperEngine *engine, const char *name, void *ptr, size_t len,
                   size_t elem_size);

/**
 * Like super_bind_slice with an explicit layout descriptor:
 * "x:f32,y:f32,vx:f32,vy:f32".
 */
int super_bind_slice_ex(SuperEngine *engine, const char *name, void *ptr, size_t len,
                      size_t elem_size, const char *layout);

/**
 * Call an exported Super function. Args/return are packed C-ABI structs matching
 * the function signature (inferred from the loaded module when possible).
 */
int super_call(SuperEngine *engine, const char *module, const char *fn_name,
             void *args_buf, void *ret_buf);

/** Call with an explicit signature descriptor. */
int super_call_sig(SuperEngine *engine, const char *module, const char *fn_name,
                 void *args_buf, void *ret_buf, const char *sig_descriptor);

#ifdef __cplusplus
}
#endif

#endif /* SUPER_H */
