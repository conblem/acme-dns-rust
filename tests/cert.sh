step certificate create root-ca root-ca.crt root-ca.key --profile root-ca --insecure --no-password

step certificate create leaf leaf.crt leaf.key --profile leaf \
  --ca ./root-ca.crt --ca-key ./root-ca.key \
  --san acme-dns-rust.com --insecure --no-password
