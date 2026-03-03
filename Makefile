BIN      := zeroclaw
RELEASE  := target/release/$(BIN)
MUSL_BIN := target/x86_64-unknown-linux-musl/release/$(BIN)

.PHONY: build small static bench install docker clean fmt lint test mcp macos nvidia metal

## Standard release build (native, glibc)
build:
	cargo build --release

## Build with MCP server support (adds tokio, serde, serde_json)
mcp:
	cargo build --release --features mcp
	@ls -lh $(RELEASE)
	@echo "MCP server enabled. Run with: $(RELEASE) --mcp"

## Build for macOS development (uses sysctl/mach APIs instead of /proc)
macos:
	cargo build --release --features macos
	@ls -lh $(RELEASE)
	@echo "macOS build. Note: process listing is stubbed."

## Build with both MCP and macOS features
macos-mcp:
	cargo build --release --features macos,mcp
	@ls -lh $(RELEASE)
	@echo "macOS + MCP build."

## Build with NVIDIA GPU monitoring (Linux, requires NVIDIA drivers)
nvidia:
	cargo build --release --features nvidia
	@ls -lh $(RELEASE)
	@echo "NVIDIA GPU monitoring enabled (requires libnvidia-ml.so)"

## Build with Metal GPU monitoring (macOS only)
metal:
	cargo build --release --features macos,metal
	@ls -lh $(RELEASE)
	@echo "Metal GPU monitoring enabled (macOS only)"

## Full macOS development build with all features
macos-full:
	cargo build --release --features macos,metal,mcp
	@ls -lh $(RELEASE)
	@echo "macOS + Metal + MCP build."

## Full Linux build with all features
linux-full:
	cargo build --release --features nvidia,mcp
	@ls -lh $(RELEASE)
	@echo "Linux + NVIDIA + MCP build."

## Release + strip (+ UPX if available)
small: build
	strip $(RELEASE)
	@if command -v upx >/dev/null 2>&1; then \
		upx --best --lzma $(RELEASE); \
		echo "UPX compressed."; \
	else \
		echo "UPX not found — skipping compression."; \
	fi
	@ls -lh $(RELEASE)

## Fully static musl binary
static:
	rustup target add x86_64-unknown-linux-musl 2>/dev/null || true
	cargo build --release --target x86_64-unknown-linux-musl
	strip $(MUSL_BIN)
	@ls -lh $(MUSL_BIN)

## Report binary size and measure startup time
bench: build
	@echo "=== Binary sizes ==="
	@ls -lh $(RELEASE) 2>/dev/null || echo "release binary not found"
	@ls -lh $(MUSL_BIN) 2>/dev/null || echo "musl binary not found (run 'make static')"
	@echo ""
	@echo "=== Startup time (--once) ==="
	@if command -v hyperfine >/dev/null 2>&1; then \
		hyperfine --warmup 3 '$(RELEASE) --once --no-color > /dev/null'; \
	else \
		time $(RELEASE) --once --no-color > /dev/null; \
	fi
	@echo ""
	@echo "=== Peak RAM usage ==="
	@/usr/bin/time -v $(RELEASE) --once --no-color > /dev/null 2>&1 | \
		grep "Maximum resident" || \
		/usr/bin/time $(RELEASE) --once --no-color > /dev/null

## Install to /usr/local/bin
install: small
	install -Dm755 $(RELEASE) /usr/local/bin/$(BIN)
	@echo "Installed to /usr/local/bin/$(BIN)"

## Static musl Docker build image
docker:
	docker run --rm \
		-v "$(PWD)":/src \
		-w /src \
		rust:alpine \
		sh -c "apk add --no-cache musl-dev && \
		       cargo build --release --target x86_64-unknown-linux-musl && \
		       strip $(MUSL_BIN)"

## Format code
fmt:
	cargo fmt

## Lint (warnings as errors)
lint:
	cargo clippy -- -D warnings

## Run tests
test:
	cargo test

## Clean build artifacts
clean:
	cargo clean

## Show binary sections breakdown (requires cargo-bloat)
bloat: build
	@cargo bloat --release --crates -n 20 2>/dev/null || \
		echo "Install with: cargo install cargo-bloat"
