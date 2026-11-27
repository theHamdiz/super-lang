ROOT := $(abspath ../..)
INCLUDE := include
NATIVE := native
TARGET_DIR := $(ROOT)/target/debug
LIBDIR := $(TARGET_DIR)

CXX ?= g++
CXXFLAGS ?= -std=c++20 -O2 -Wall -Wextra -I$(INCLUDE) -I$(NATIVE)
LDFLAGS ?= -L$(LIBDIR) -Wl,-rpath,$(LIBDIR) -lsuper_ffi -ldl -lpthread -lm -lrt -lgcc_s

.PHONY: all lib native bindgen run test clean headers

all: headers lib native bindgen host

headers:
	@mkdir -p include
	@cp -f $(ROOT)/include/super.h include/super.h
	@cp -f $(ROOT)/include/super.hpp include/super.hpp

lib:
	cd $(ROOT) && cargo build -p super-ffi
	cd $(ROOT) && cargo build -p super-cli

native: $(NATIVE)/damage.cpp $(NATIVE)/damage.hpp $(NATIVE)/damage_c.h
	mkdir -p target
	$(CXX) $(CXXFLAGS) -c $(NATIVE)/damage.cpp -o target/damage.o

bindgen: lib
	mkdir -p target
	$(ROOT)/target/debug/super bindgen src/combat.sr --out target/combat_generated.h

host: main.cpp lib native headers
	mkdir -p target
	$(CXX) $(CXXFLAGS) main.cpp target/damage.o -o target/host $(LDFLAGS)

run: host
	LD_LIBRARY_PATH=$(LIBDIR):$$LD_LIBRARY_PATH ./target/host src/combat.sr

test: run

clean:
	rm -rf target
