FROM alpine
RUN apk add --no-cache openssl

COPY /target/x86_64-unknown-linux-musl/release/acme-dns-rust /

ENTRYPOINT ["/acme-dns-rust"]
