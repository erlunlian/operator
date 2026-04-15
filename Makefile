SHELL := /bin/zsh -l
.PHONY: help dev run release dmg install open clean release-tag

BUMP ?= patch

help:
	@echo "make dev              Dev build with auto-reload on save"
	@echo "make run              Single build and run"
	@echo "make release          Build optimized .app bundle"
	@echo "make dmg              Build release and create DMG installer"
	@echo "make release-tag      Bump version, commit, tag, and push (BUMP=patch|minor|major)"
	@echo "make open             Build release and open the app"
	@echo "make install          Build release and copy to /Applications"
	@echo "make clean            Remove all build artifacts"

# Dev with auto-reload on save + full backtraces + info logging
dev:
	RUST_LOG=info RUST_BACKTRACE=1 cargo watch -x run

# Single build + run
run:
	RUST_LOG=info RUST_BACKTRACE=1 cargo run

# Release .app bundle
release:
	./script/build-release

# Build release and create DMG installer
dmg: release
	./script/create-dmg

# Bump version in Cargo.toml, commit, create git tag, and push.
# Usage: make release-tag BUMP=minor  (default: patch)
release-tag:
	@current=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	IFS='.' read -r major minor patch <<< "$$current"; \
	case "$(BUMP)" in \
		major) major=$$((major + 1)); minor=0; patch=0 ;; \
		minor) minor=$$((minor + 1)); patch=0 ;; \
		patch) patch=$$((patch + 1)) ;; \
		*) echo "Unknown BUMP=$(BUMP). Use major, minor, or patch." && exit 1 ;; \
	esac; \
	next="$$major.$$minor.$$patch"; \
	sed -i '' "s/^version = \"$$current\"/version = \"$$next\"/" Cargo.toml; \
	cargo check --quiet 2>/dev/null; \
	git add Cargo.toml Cargo.lock; \
	git commit -m "Bump version to $$next"; \
	git tag "v$$next"; \
	git push origin HEAD "v$$next"; \
	echo "Released v$$next"

# Copy to /Applications
install:
	@MAKE_INSTALL=1 ./script/build-release
	@cp -r target/release/Operator.app /Applications/
	@echo ""
	@echo "Installed to /Applications/Operator.app"
	@echo "To run:  open -n /Applications/Operator.app"

# Open the installed app (-n forces new instance even if already running)
open:
	open -n /Applications/Operator.app

# Clean build artifacts
clean:
	cargo clean
	rm -rf target/release/Operator.app
	rm -f target/release/Operator.dmg
