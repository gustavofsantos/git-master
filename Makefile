# Pinned here and in .github/workflows/ci.yml. Installed under target/, so it
# never touches ~/.cargo/bin.
CARGO_AUDIT_VERSION := 0.22.2
CARGO_AUDIT := target/tools/bin/cargo-audit

.PHONY: build run install uninstall test lint fmt audit check hooks clean

build:
	cargo build --release -p gitmaster

run:
	cargo run --release -p gitmaster -- $(ARGS)

# PREFIX is where `make install` puts bin/gitmaster. Unset, cargo's own install
# root applies: $CARGO_INSTALL_ROOT, else ~/.cargo, so ~/.cargo/bin/gitmaster.
# On Linux it also installs the icons and a desktop entry whose Exec is the
# absolute path of the binary, as install.sh does. On macOS, GitMaster.app under
# APPDIR.
PREFIX ?=
DATADIR ?= $(or $(XDG_DATA_HOME),$(HOME)/.local/share)
APPDIR ?= $(HOME)/Applications
ROOT := $(or $(PREFIX),$(CARGO_INSTALL_ROOT),$(HOME)/.cargo)
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo 0.0.0)
STAGE := target/stage

install:
	cargo install --path crates/gitmaster --locked --force --root '$(ROOT)'
	rm -rf '$(STAGE)'
	scripts/stage-release.sh '$(ROOT)/bin/gitmaster' '$(STAGE)' '$(VERSION)'
ifeq ($(shell uname -s),Darwin)
	mkdir -p '$(APPDIR)'
	rm -rf '$(APPDIR)/GitMaster.app'
	cp -R '$(STAGE)/GitMaster.app' '$(APPDIR)/GitMaster.app'
else
	mkdir -p '$(DATADIR)/applications' '$(DATADIR)/icons'
	cp -R '$(STAGE)/share/icons/hicolor' '$(DATADIR)/icons/'
	sed 's|^Exec=.*|Exec=$(ROOT)/bin/gitmaster|' '$(STAGE)/share/applications/gitmaster.desktop' >'$(DATADIR)/applications/gitmaster.desktop'
endif

uninstall:
	cargo uninstall gitmaster --root '$(ROOT)'
ifeq ($(shell uname -s),Darwin)
	rm -rf '$(APPDIR)/GitMaster.app'
else
	rm -f '$(DATADIR)/applications/gitmaster.desktop'
	find '$(DATADIR)/icons/hicolor' -name gitmaster.png -delete 2>/dev/null || true
endif

test:
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings
	cargo fmt --all --check

fmt:
	cargo fmt --all

$(CARGO_AUDIT):
	cargo install cargo-audit --locked --version $(CARGO_AUDIT_VERSION) --root target/tools

audit: $(CARGO_AUDIT)
	$(CARGO_AUDIT) audit

check: lint audit test

hooks:
	git config core.hooksPath .githooks

clean:
	cargo clean
