# nknown-server : API REST + interface web, données dans le volume /data.
#
#   docker build -t nknown .
#   docker build -t nknown --build-arg FEATURES=neural .   # avec l'analyseur XLM-R (CPU)
#
# Modèles (lexiques .lex, tagueurs .tag, analyseur) : à déposer dans /data/models.

ARG RUST_VERSION=1

FROM rust:${RUST_VERSION}-slim-trixie AS build
ARG FEATURES=""
# Interface web : WebAssembly (même version de wasm-bindgen que le crate), wasm-opt (binaryen).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends binaryen \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && cargo install --locked wasm-bindgen-cli --version 0.2.128
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p nknown-server --bin nknown-server \
        ${FEATURES:+--features "$FEATURES"} \
 && cp target/release/nknown-server /usr/local/bin/ \
 && crates/nknown-web/build.sh /srv/web

FROM debian:trixie-slim
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin nknown \
 && mkdir -p /data /srv/web \
 && chown nknown:nknown /data
COPY --from=build /usr/local/bin/nknown-server /usr/local/bin/nknown-server
COPY --from=build /srv/web /srv/web

ENV NKNOWN_DATA=/data \
    NKNOWN_ADDR=0.0.0.0:8080 \
    NKNOWN_WEB=/srv/web
USER nknown
VOLUME /data
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/nknown-server"]
