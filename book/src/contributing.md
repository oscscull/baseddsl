# Changing Based

Keep files and functions focused on one responsibility. Add features only within
the issue's scope. Keep documentation to what readers need to use or understand
Based; evidence and work status belong in the issue or PR.

```sh
make check-fast  # iteration: formatting and infra-free workspace tests
make check       # execution changes: full lint/tests, live databases, and examples
```

Generator changes also run `make ci-generated-consumer-sqlite` and the two server
consumer targets. Optional Cargo integration runs `make ci-cargo-generation`.
Database TLS verification runs `make ci-database-tls` and needs Docker/OpenSSL.

Native packaging can be checked without publication:

```sh
cargo build --release --locked -p based-cli -p based-lsp
cargo run -p based-ci -- package --binary-dir target/release \
  --output /tmp/based-assets --target YOUR_RUSTC_HOST
```

The release workflow produces matching native archives and VSIX with checksums.
Only an explicit matching version tag creates a draft for owner review.
