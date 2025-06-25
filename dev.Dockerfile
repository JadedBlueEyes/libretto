ARG RUST_VERSION=1
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-slim-${DEBIAN_VERSION}

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Node.js and pnpm
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && corepack enable

# Install Rust development tools
# watchexec-cli official builds have an incompatible glibc, so we don't use them.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    curl --retry 5 -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall --no-confirm watchexec-cli --disable-strategies crate-meta-data \
    && cargo binstall --no-confirm sccache

WORKDIR /app

# Copy package files and install Node dependencies
COPY package.json pnpm-lock.yaml ./

RUN --mount=type=cache,id=pnpm,target=/root/.local/share/pnpm/store \
    pnpm install

# Copy source code
COPY . .

EXPOSE 3000

# Run both commands in parallel
CMD ["sh", "-c", "pnpm run watch & watchexec -r -e rs,j2,toml cargo run -- -vv"]
