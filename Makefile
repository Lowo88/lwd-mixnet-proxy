.PHONY: build test lint fmt fmt-fix verify image

build:
	cargo build

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --check

fmt-fix:
	cargo fmt

verify: fmt lint build test

# Release binaries for both halves, in the image they are meant to run in.
image:
	docker build -t lwd-mixnet-proxy .
