ROOT := $(abspath ../..)
# Prefer the vendored copy so this example is self-contained; fall back to repo include/.
INCLUDE := include
NATIVE := native
TARGET_DIR := $(ROOT)/target/debug
LIBDIR := $(TARGET_DIR)

CC ?= gcc
CFLAGS ?= -std=c17 -O2 -Wall -Wextra -I$(INCLUDE) -I$(NATIVE)
LDFLAGS ?= -L$(LIBDIR) -Wl,-rpath,$(LIBDIR) -lsuper_ffi -ldl -lpthread -lm -lrt -lgcc_s

.PHONY: all lib native run test clean headers

all: headers lib native host

headers:
	@mkdir -p include
	@cp -f $(ROOT)/include/super.h include/super.h

lib:
	cd $(ROOT) && cargo build -p super-ffi

native: $(NATIVE)/mathlib.c $(NATIVE)/mathlib.h
	mkdir -p target
	$(CC) $(CFLAGS) -c $(NATIVE)/mathlib.c -o target/mathlib.o

host: main.c lib native headers
	mkdir -p target
	$(CC) $(CFLAGS) main.c target/mathlib.o -o target/host $(LDFLAGS)

run: host
	LD_LIBRARY_PATH=$(LIBDIR):$$LD_LIBRARY_PATH ./target/host src/physics.sr

test: run

clean:
	rm -rf target
