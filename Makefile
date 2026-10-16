# Pinned here and in .github/workflows/ci.yml. Installed under target/, so it
# never touches ~/.cargo/bin.
CARGO_AUDIT_VERSION := 0.22.2
CARGO_AUDIT := target/tools/bin/cargo-audit

.PHONY: build run test lint fmt audit check hooks clean

build:
	cargo build --release -p gitmaster

run:
	cargo run --release -p gitmaster -- $(ARGS)

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
