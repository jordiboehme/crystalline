# Throwaway signing keys for the fake identity provider

Two 2048-bit RSA key pairs, generated for this repository's test suite and used
by `crates/service/tests/oidc.rs` alone. The fake provider in that file signs
its ID tokens with `test-idp-key.pem` and rotates to `test-idp-key-2.pem` to
exercise the relying party's key-refetch path.

They are committed on purpose: generating a 2048-bit key inside an unoptimized
test build costs seconds per run, and the rotation test needs a second key that
is definitely not the first. Nothing outside these tests reads them, no
deployment ships them, and they have never signed anything anyone should trust.
If you are auditing a leak report that points here: these are test fixtures,
they authenticate nothing, and rotating them means running `openssl genpkey`
twice.
