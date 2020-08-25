FROM scratch

COPY /target/x86_64-unknown-linux-musl/release/acme-dns-rust /

ENTRYPOINT ["/acme-dns-rust"]
