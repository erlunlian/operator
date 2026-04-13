.PHONY: dev run release install clean

# Dev with auto-reload on save + full backtraces
dev:
	RUST_BACKTRACE=1 cargo watch -x run

# Single build + run
run:
	RUST_BACKTRACE=1 cargo run

# Release .app bundle
release:
	./script/build-release

# Copy to /Applications
install: release
	cp -r target/release/Operator.app /Applications/
	@echo "Installed to /Applications/Operator.app"

# Open the built .app
open: release
	open target/release/Operator.app

# Clean build artifacts
clean:
	cargo clean
	rm -rf target/release/Operator.app
