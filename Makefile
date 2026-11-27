# Super toolchain build & install
#
#   make install   — release-optimized binaries + stdlib → ~/.super
#   make build     — release build only (target/release/)
#   make test      — workspace + example tests
#   make uninstall — remove installed shims/binaries
#   make clean*    — wipe build artifacts (see help)

SUPER_HOME  ?= $(HOME)/.super
PREFIX      ?= $(SUPER_HOME)
BIN_DIR     ?= $(PREFIX)/bin
CARGO       ?= cargo
PROFILE     ?= release
ROOT        := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
CLEAN_SH    := $(ROOT)/scripts/clean.sh
ZED_INSTALL := $(ROOT)/editors/zed/install.sh

.PHONY: all build install install-zed uninstall test examples check bench \
	stdlib-counts fuzz-template ci-stdlib \
	clean clean-debug clean-release clean-cargo clean-zed clean-grammar clean-all distclean help

all: install

help:
	@echo "Super Makefile targets:"
	@echo "  make install         Build heavily optimized tooling and install to $(BIN_DIR)"
	@echo "  make install-zed     Build + register Super Zed extension (auto-detects Zed data dir)"
	@echo "  make build                 cargo build --release — opt-level=3, fat LTO, stripped"
	@echo "  make PROFILE=size build    smallest binary (opt-level=z, fat LTO)"
	@echo "  make PROFILE=fast build    same peak as release"
	@echo "  make test            cargo test --workspace && super test examples/"
	@echo "  make examples        Run all example tests via installed or local super"
	@echo "  make bench           Release 100M ECS + HTTP benches"
	@echo "  make uninstall       Remove super / super-lsp / super-mcp from $(BIN_DIR)"
	@echo ""
	@echo "Stdlib: use std.http / use std.ecs / use std.collections  (SUPER_STD overrides path)"
	@echo ""
	@echo "Clean:"
	@echo "  make clean-debug     Remove target/debug only"
	@echo "  make clean-release   Remove target/release only"
	@echo "  make clean-cargo     cargo clean + prune entire target/"
	@echo "  make clean-zed       Remove editors/zed/target"
	@echo "  make clean-grammar   Remove tree-sitter build/wasm artifacts"
	@echo "  make clean           Alias for clean-cargo (workspace target/)"
	@echo "  make clean-all       Everything: cargo + zed + grammar (alias: distclean)"
	@echo "  ./scripts/clean.sh [debug|release|cargo|zed|grammar|all]"

ifeq ($(PROFILE),release)
CARGO_PROFILE_ARGS := --release
else
CARGO_PROFILE_ARGS := --profile $(PROFILE)
endif

build:
	$(CARGO) build --workspace $(CARGO_PROFILE_ARGS)

# Optimized install: LTO + opt-level=3, binaries + stdlib under ~/.super (or $PREFIX).
install:
	@chmod +x "$(ROOT)/scripts/install.sh"
	@SUPER_HOME="$(SUPER_HOME)" SUPER_PREFIX="$(PREFIX)" SUPER_BIN_DIR="$(BIN_DIR)" "$(ROOT)/scripts/install.sh"
	@command -v super >/dev/null && super --version || true

uninstall:
	@rm -f "$(BIN_DIR)/super" "$(BIN_DIR)/super-lsp" "$(BIN_DIR)/super-mcp"
	@echo "Removed super, super-lsp, and super-mcp from $(BIN_DIR)"
	@echo "Stdlib left at $(SUPER_HOME)/lib/stdlib (remove manually if desired)"

# Detect Zed CLI/data/config dirs, build wasm + grammar, register extension + LSP.
install-zed:
	@chmod +x "$(ZED_INSTALL)"
	@"$(ZED_INSTALL)"

test:
	$(CARGO) test --workspace
	@if command -v super >/dev/null 2>&1; then \
		super test examples/; \
	else \
		$(CARGO) run -p super-cli -- test examples/; \
	fi

examples:
	@if command -v super >/dev/null 2>&1; then \
		super test examples/; \
	else \
		$(CARGO) run -p super-cli --release -- test examples/; \
	fi

check:
	@if command -v super >/dev/null 2>&1; then \
		super check examples/01_space_shooter && \
		super check examples/02_task_api && \
		super check examples/03_log_analyzer; \
	else \
		$(CARGO) run -p super-cli --release -- check examples/01_space_shooter; \
	fi

# A4 / Appendix B: tree, SPECIFICATION.md, and counting script must agree.
# Also run by `.github/workflows/stdlib-gates.yml` (CI fails on mismatch).
stdlib-counts:
	@python3 "$(ROOT)/scripts/count_stdlib.py"
	@python3 "$(ROOT)/scripts/assert_secret_vault_names.py"

# Template parser fuzz. CI sets SUPER_TEMPLATE_FUZZ_SECS=600 (10 minutes).
fuzz-template:
	@SUPER_TEMPLATE_FUZZ_SECS=$${SUPER_TEMPLATE_FUZZ_SECS:-30} python3 "$(ROOT)/scripts/fuzz_template.py"

# Local mirror of CI stdlib gates (fuzz duration still overridden by env).
ci-stdlib: stdlib-counts
	@$(CARGO) test -p super-frontend --test template_diags --lib template -- --nocapture
	@if command -v super >/dev/null 2>&1; then \
		super test stdlib/html/template.sr && \
		super test stdlib/sec/secret.sr && \
		super verify stdlib/html/template.sr && \
		super verify stdlib/sec/secret.sr; \
	else \
		$(CARGO) run -p super-cli --release -- test stdlib/html/template.sr; \
	fi

bench:
	$(CARGO) run -p super-cli --release -- bench

# --- clean -------------------------------------------------------------------

clean-debug:
	@$(CLEAN_SH) debug

clean-release:
	@$(CLEAN_SH) release

clean-cargo:
	@$(CLEAN_SH) cargo

clean-zed:
	@$(CLEAN_SH) zed

clean-grammar:
	@$(CLEAN_SH) grammar

# Default `clean` wipes the workspace cargo target (debug + release + rest).
clean: clean-cargo

clean-all distclean:
	@$(CLEAN_SH) all
