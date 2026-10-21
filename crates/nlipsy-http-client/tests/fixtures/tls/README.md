# TLS test fixtures

The test PKI the `Dialer` suite in `src/transport.rs` (`hyper_dialer_tests`) runs its
`https` exchanges against, read with `include_bytes!`:

| file | what it is |
|---|---|
| `ca.der` | the CA certificate: ECDSA P-256, `basicConstraints = critical, CA:TRUE`, key usage `keyCertSign, cRLSign` |
| `leaf.der` | a leaf for DNS `nlip.test` and nothing else — the name is in the SAN only, not in a CN — with EKU `serverAuth`, signed by the CA |
| `leaf.key.der` | the leaf's private key, PKCS#8 DER |

All three are valid from 2026-01-01 to 2126-01-01 (notAfter is a GeneralizedTime), so
nothing here expires in practice. P-256 with SHA-256 because rustls's `ring` provider both
signs and verifies with it. The CA's own key is thrown away by the recipe below: nothing
needs to sign anything new, and regenerating makes a new CA along with everything else.

**`leaf.key.der` is a test-only key that protects nothing**: it is published here on
purpose, and its certificate is trusted only by tests that add `ca.der` themselves — no
deployment should ever add that root.

## Regenerating

OpenSSL 3.4 or newer, which is where `-not_before` and `-not_after` arrived (Homebrew's
`openssl@3` is; macOS's LibreSSL has neither). Run from this directory; it replaces all
three files at once:

```sh
sh -eu <<'EOF'
OPENSSL=${OPENSSL:-/opt/homebrew/opt/openssl@3/bin/openssl}
NOT_BEFORE=20260101000000Z
NOT_AFTER=21260101000000Z
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf '%s\n' \
  'basicConstraints = critical, CA:TRUE' \
  'keyUsage = critical, keyCertSign, cRLSign' \
  'subjectKeyIdentifier = hash' > "$work/ca.ext"
printf '%s\n' \
  'basicConstraints = critical, CA:FALSE' \
  'keyUsage = critical, digitalSignature' \
  'extendedKeyUsage = serverAuth' \
  'subjectAltName = DNS:nlip.test' \
  'subjectKeyIdentifier = hash' \
  'authorityKeyIdentifier = keyid' > "$work/leaf.ext"

"$OPENSSL" genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -pkeyopt ec_param_enc:named_curve -out "$work/ca.key"
"$OPENSSL" x509 -new -key "$work/ca.key" -subj '/O=nlipsy test fixtures/CN=nlipsy test CA' \
  -not_before "$NOT_BEFORE" -not_after "$NOT_AFTER" -extfile "$work/ca.ext" \
  -outform DER -out ca.der

"$OPENSSL" genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 \
  -pkeyopt ec_param_enc:named_curve -out "$work/leaf.key"
"$OPENSSL" pkcs8 -topk8 -nocrypt -in "$work/leaf.key" -outform DER -out leaf.key.der
"$OPENSSL" req -new -key "$work/leaf.key" -subj '/O=nlipsy test fixtures' -out "$work/leaf.csr"
"$OPENSSL" x509 -req -in "$work/leaf.csr" -CA ca.der -CAkey "$work/ca.key" \
  -not_before "$NOT_BEFORE" -not_after "$NOT_AFTER" -extfile "$work/leaf.ext" \
  -outform DER -out leaf.der
EOF
```

The `pkcs8 -topk8` step is not decoration: `genpkey -outform DER` writes an EC key as
SEC1 (`ECPrivateKey`), and rustls is handed the key as PKCS#8. Then run
`cargo test -p nlipsy-http-client --features hyper`.
