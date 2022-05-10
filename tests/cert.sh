step certificate create root-ca root-ca.crt root-ca.key --profile root-ca --kty RSA --insecure --no-password --force

step certificate create leaf leaf.crt leaf.key --profile leaf \
  --ca ./root-ca.crt --ca-key ./root-ca.key \
  --san acme-dns-rust.com --kty RSA --insecure --no-password --force
