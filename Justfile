# Path to the freshly built release binary.
# .cargo/config.toml pins `target = "aarch64-apple-darwin"`, so cargo
# nests the binary under target/<triple>/release/ rather than target/release/.
release_bin := `cargo metadata --no-deps --format-version 1 | jq -r '.target_directory'` + "/aarch64-apple-darwin/release/red-cli"

# Default: build the production binary.
default: build

# Production build: optimized release binary for the native target.
# Uses [profile.release] in Cargo.toml (opt-level=3, lto=true,
# codegen-units=1, strip=true). Builds only the red-cli binary —
# fuzz/ is excluded from the workspace (nightly-only).
build:
    cargo build --release -p red-cli

# Stage the stripped binary into dist/ with version + arch suffix.
dist: build
    @mkdir -p dist
    @cp {{release_bin}} dist/red-cli-$(cargo pkgid -p red-cli | cut -d# -f2 | cut -d: -f2)-{{arch()}}
    @ls -lh dist/

# Smoke-test the freshly built binary.
check: build
    {{release_bin}} --version
    {{release_bin}} examples/hello.red

# Remove production build artifacts.
clean:
    cargo clean --release
    rm -rf dist
