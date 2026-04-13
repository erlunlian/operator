SHELL := /bin/zsh -l
.PHONY: help dev run release install open clean

help:
	@echo "make dev       Dev build with auto-reload on save"
	@echo "make run       Single build and run"
	@echo "make release   Build optimized .app bundle"
	@echo "make open      Build release and open the app"
	@echo "make install   Build release and copy to /Applications"
	@echo "make clean     Remove all build artifacts"

# Dev with auto-reload on save + full backtraces + info logging
dev:
	RUST_LOG=info RUST_BACKTRACE=1 cargo watch -x run

# Single build + run
run:
	RUST_LOG=info RUST_BACKTRACE=1 cargo run

# Release .app bundle
release:
	./script/build-release

# Copy to /Applications
install: release
	cp -r target/release/Operator.app /Applications/

# Open the installed app (-n forces new instance even if already running)
open:
	open -n /Applications/Operator.app

# Clean build artifacts
clean:
	cargo clean
	rm -rf target/release/Operator.app
