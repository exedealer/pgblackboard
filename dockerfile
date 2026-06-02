# https://hub.docker.com/_/rust/tags?name=alpine
FROM rust:1.96-alpine3.23 AS dev
RUN apk add --no-cache make esbuild pkgconf openssl-dev
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
