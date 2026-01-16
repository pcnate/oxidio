# Oxidio Build Container
# Supports building for Linux (native) and Windows (cross-compile)

FROM rustlang/rust:nightly-bookworm AS builder

# Install cross-compilation dependencies for Windows
RUN apt-get update && apt-get install -y \
    # Windows cross-compilation
    gcc-mingw-w64-x86-64 \
    # Audio dependencies for Linux builds
    libasound2-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Add Windows target
RUN rustup target add x86_64-pc-windows-gnu

# Set up cargo config for Windows cross-compilation
RUN mkdir -p /root/.cargo && echo '\
[target.x86_64-pc-windows-gnu]\n\
linker = "x86_64-w64-mingw32-gcc"\n\
' > /root/.cargo/config.toml

WORKDIR /app

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock* ./
COPY crates/oxidio-core/Cargo.toml ./crates/oxidio-core/
COPY crates/oxidio-cli/Cargo.toml ./crates/oxidio-cli/

# Create dummy source files for dependency compilation
RUN mkdir -p crates/oxidio-core/src crates/oxidio-cli/src && \
    echo "pub fn dummy() {}" > crates/oxidio-core/src/lib.rs && \
    echo "fn main() {}" > crates/oxidio-cli/src/main.rs

# Build dependencies (cached layer)
RUN cargo build --release || true
RUN cargo build --release --target x86_64-pc-windows-gnu || true

# Remove dummy files
RUN rm -rf crates/oxidio-core/src crates/oxidio-cli/src

# Copy actual source
COPY crates/ ./crates/

# Build for Linux
FROM builder AS build-linux
RUN cargo build --release
RUN strip target/release/oxidio

# Build for Windows
FROM builder AS build-windows
RUN cargo build --release --target x86_64-pc-windows-gnu
RUN x86_64-w64-mingw32-strip target/x86_64-pc-windows-gnu/release/oxidio.exe

# Output stage - collects both binaries
FROM debian:bookworm-slim AS output

COPY --from=build-linux /app/target/release/oxidio /output/linux/oxidio
COPY --from=build-windows /app/target/x86_64-pc-windows-gnu/release/oxidio.exe /output/windows/oxidio.exe

CMD ["echo", "Binaries available in /output/linux and /output/windows"]
