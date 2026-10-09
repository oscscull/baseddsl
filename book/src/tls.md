# Verified database connections

The default installed CLI enables Rustls for Postgres and MariaDB/MySQL. Embedded
applications opt in with `based-runtime` features `postgres,tls-rustls` or
`mariadb,tls-rustls`. The `tls-rustls` feature alone enables no driver; SQLite-only
consumers need neither it nor TLS dependencies. The server quickstarts expose a
`tls` feature: run `cargo run --features tls` with the settings below.

TLS availability does **not** make every connection certificate-verified. SQLx's
server-driver defaults prefer TLS and can allow weaker negotiation. Explicitly
require identity verification in production connection URLs:

```text
postgres://USER:PASSWORD@db.example.com:5432/app?sslmode=verify-full&sslrootcert=/absolute/path/ca.pem
mysql://USER:PASSWORD@db.example.com:3306/app?ssl-mode=VERIFY_IDENTITY&ssl-ca=/absolute/path/ca.pem
```

These SQLx options require encryption, trust in the configured CA, and a matching
server hostname. Use the DNS name covered by the certificate's subject alternative
names. An IP address needs a corresponding IP SAN. An untrusted CA, wrong hostname,
or server without TLS must fail; `require`/`REQUIRED` alone do not establish the same
identity guarantee. The CA path is
resolved by SQLx against process cwd: use an absolute path, and URL-encode reserved
characters in paths and credentials. Keep URLs in deployment secrets or ignored
local `.env` files rather than checking credentials into source control.

The same URL applies to `based serve` and `based migrate apply/status` through
`--database-url` or the documented [local connection configuration](configuration.md).
For multiple shards, configure verification on every server URL. HTTP TLS termination is a separate deployment concern.

For disposable loopback servers that deliberately have no TLS, explicitly select
Postgres `sslmode=disable` or MariaDB `ssl-mode=DISABLED`. Never carry that setting
to a remote production server.

## Application-owned pools

An embedded application may configure SQLx's `PgConnectOptions` with
`PgSslMode::VerifyFull` / `ssl_root_cert`, or `MySqlConnectOptions` with
`MySqlSslMode::VerifyIdentity` / `ssl_ca`, then build its pool and supply it through
`PgRouter::from_pool` or `ShardRouter::from_pool`. The application's SQLx dependency
must select a TLS backend (or enable `based-runtime/tls-rustls`, which unifies it).
The pool owner also owns connection limits and timeouts. Based does not replace
SQLx's certificate validation or create a custom cryptographic protocol.
