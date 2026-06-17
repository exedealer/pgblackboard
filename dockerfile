# https://hub.docker.com/_/rust/tags?name=alpine
FROM rust:1.96-alpine3.23 AS dev
RUN apk add --no-cache make esbuild pkgconf openssl-dev \
  && apk add --no-cache --repository https://dl-cdn.alpinelinux.org/alpine/edge/testing biome

ADD --unpack \
  https://github.com/oxc-project/oxc/releases/download/apps_v1.69.0/oxlint-x86_64-unknown-linux-musl.tar.gz \
  https://github.com/oxc-project/oxc/releases/download/apps_v1.69.0/oxfmt-x86_64-unknown-linux-musl.tar.gz \
  /usr/local/bin
RUN mv /usr/local/bin/oxlint* /usr/local/bin/oxlint \
  && mv /usr/local/bin/oxfmt* /usr/local/bin/oxfmt

WORKDIR /app
EXPOSE 7890
COPY Cargo.toml Cargo.lock ./
RUN cargo fetch
ENV RUSTFLAGS="-C target-feature=-crt-static"
ENV RUST_BACKTRACE=1
# ENV TOKIO_WORKER_THREADS=1

FROM dev AS build
RUN mkdir server \
  && touch server/main.rs \
  && cargo build --release --frozen \
  || true
COPY . .
RUN make build

FROM alpine:3.23
LABEL org.opencontainers.image.authors="exe-dealer@yandex.kz"
RUN apk add --no-cache libgcc \
  && adduser -HDu 1000 web # https://man.archlinux.org/man/busybox.1.en#adduser
WORKDIR /tmp
EXPOSE 7890
ENV RUST_BACKTRACE=1
CMD ["pgbb", "postgres://postgres:5432"]
COPY --from=build /app/target/release/pgbb /usr/local/bin/
USER web
