# syntax=docker/dockerfile:1
ARG BINARY_SOURCE=build

# Local source build. Each platform compiles for its native musl target.
FROM rust:1.90.0-alpine3.22 AS build
RUN apk add --no-cache musl-dev python3
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY scripts/third_party_notices.py ./scripts/
COPY licenses ./licenses
RUN cargo fetch --locked \
    && cargo build --release --locked -p oche-cli \
    && mkdir -p /out \
    && cp target/release/oche-core /out/oche-core \
    && python3 scripts/third_party_notices.py \
       --target "$(rustc -vV | sed -n 's/^host: //p')" \
       --output /out/THIRD_PARTY_NOTICES.txt

# Releases supply the same tested binaries that are attached to GitHub Releases.
FROM scratch AS prebuilt
ARG TARGETARCH
COPY dist/${TARGETARCH}/ /out/

FROM ${BINARY_SOURCE} AS binary

# No compiler, package manager, shell, or runtime downloads in the final image.
FROM scratch
COPY --from=binary /out/oche-core /usr/local/bin/oche-core
COPY --from=binary /out/THIRD_PARTY_NOTICES.txt /usr/share/doc/ochecore/THIRD_PARTY_NOTICES.txt
COPY LICENSE NOTICE /usr/share/doc/ochecore/
COPY --chown=65532:65532 docker/data/ /data/
USER 65532:65532
ENV XDG_CONFIG_HOME=/data TERM=xterm-256color PATH=/usr/local/bin
VOLUME ["/data"]
EXPOSE 3180
LABEL org.opencontainers.image.title="OcheCore" \
      org.opencontainers.image.description="Offline board control service and terminal UI" \
      org.opencontainers.image.licenses="AGPL-3.0-only"
ENTRYPOINT ["/usr/local/bin/oche-core"]
CMD ["--bind", "0.0.0.0", "run"]
