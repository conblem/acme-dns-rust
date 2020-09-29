[![Build](https://github.com/conblem/acme-dns-rust/workflows/Rust/badge.svg)](https://github.com/conblem/acme-dns-rust/actions)
[![Dependabot Status](https://api.dependabot.com/badges/status?host=github&repo=conblem/acme-dns-rust)](https://dependabot.com)

# Acme DNS Rust
### WIP!

This is an implementation based on the awesome Go Project [Acme DNS](https://github.com/joohoi/acme-dns) written by @joohoi.
It aims to be API compatible with the original implementation.

## Configuration
By default a configuration file is expected in the working directory with the name config.toml .

**Example config:**
```toml
[general]
dns = "0.0.0.0:8053"
db = "postgresql://postgres:mysecretpassword@localhost/postgres"
acme = "https://acme-staging-v02.api.letsencrypt.org/directory"
name = "acme.example.com"

[api]
http = "0.0.0.0:8080"
https = "0.0.0.0:8081"
```

It is possible to pass a diferent path as the first argument to the executable.
```bash
./acme-dns-rust different_name.toml
```