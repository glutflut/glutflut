# Elements server + web app in one image (basemap and videos come from the
# host, see docs/deploy.md).
# Build from the repo root:  podman build -f deploy/Dockerfile -t elements .
# For ARM (aarch64) hosts, on an x86 machine without emulation:
#   podman build --platform linux/arm64 -f deploy/Dockerfile -t elements:arm64 .
# The build stages run on the build machine (the server is cross-compiled for
# arm64); the runtime stage runs no commands, its CA certificates and user
# come from the builder.

# ---- server ----
FROM --platform=$BUILDPLATFORM docker.io/library/rust:1-slim AS server
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends perl make pkg-config ca-certificates \
      $([ "$TARGETARCH" = arm64 ] && echo gcc-aarch64-linux-gnu libc6-dev-arm64-cross) \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home elements
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN if [ "$TARGETARCH" = arm64 ]; then \
      rustup target add aarch64-unknown-linux-gnu \
      && cargo build --release --target aarch64-unknown-linux-gnu -p elements-server --bin elements-server \
      && cp target/aarch64-unknown-linux-gnu/release/elements-server /elements-server; \
    else \
      cargo build --release -p elements-server --bin elements-server \
      && cp target/release/elements-server /elements-server; \
    fi

# ---- web (plain files) ----
FROM --platform=$BUILDPLATFORM docker.io/library/node:22-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
# Shown in the web app, e.g. --build-arg ELEMENTS_VERSION="$(git describe --always) $(date -u +%F)".
ARG ELEMENTS_VERSION=dev
ENV ELEMENTS_VERSION=$ELEMENTS_VERSION
RUN npm run build

# ---- runtime (no RUN, so no emulation for another architecture) ----
FROM docker.io/library/debian:trixie-slim
COPY --from=server /etc/ssl/certs /etc/ssl/certs
COPY --from=server /usr/share/ca-certificates /usr/share/ca-certificates
COPY --from=server /etc/passwd /etc/group /etc/
COPY --from=server /elements-server /usr/local/bin/elements-server
COPY --from=web /web/dist /srv/web
# Game rules (docs/configuration.md), plus an instance's own sets from
# rules.local/ if there (not in git); single values with ELEMENTS_RULES_OVERRIDE.
COPY rules rules.loca[l] /srv/rules/
USER elements
ARG ELEMENTS_VERSION=dev
ENV ELEMENTS_VERSION=$ELEMENTS_VERSION \
    ELEMENTS_WEB_BIND=0.0.0.0:5080 \
    ELEMENTS_WEB_DIR=/srv/web \
    ELEMENTS_STATIC_DIR=/srv/static \
    ELEMENTS_RULES=/srv/rules/default.toml
EXPOSE 5080
ENTRYPOINT ["elements-server"]
