[![Build](https://github.com/conblem/acme-dns-rust/workflows/Rust/badge.svg)](https://github.com/conblem/acme-dns-rust/actions)
[![codecov](https://codecov.io/gh/conblem/acme-dns-rust/branch/dev/graph/badge.svg)](https://codecov.io/gh/conblem/acme-dns-rust)
[![crates.io](https://img.shields.io/crates/v/acme-dns-rust)](https://crates.io/crates/acme-dns-rust)

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

[records."acme.example.com"]
A = [100, "1.1.1.1", "2.2.2.2"]
TXT = [100, "First", "Record"]

[records."acme2.example.com"]
TXT = [100, "Hallo", "World"]
CNAME = [100, "lb.cloudflare.com"]

[api]
http = "0.0.0.0:8080"
# Adding a true after the addr activates proxy protocol for a listener
https = ["0.0.0.0:8081", true]
#Every listener is optional this line could be removed completely
prom = "0.0.0.0:8081"
```

It is possible to pass a diferent path as the first argument to the executable.
```bash
./acme-dns-rust different_name.toml
```

### Records configuration
Acme DNS supports serving static DNS Records.

Currently supported records are:
* TXT
* A
* CNAME

CName records get resolved by the default OS DNS configuration.
For obvious reasons CNAME records don't support multiple values, unlike TXT and A records.