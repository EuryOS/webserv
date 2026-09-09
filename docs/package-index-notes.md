# Package index notes

The first `webserv` pilot answers whether architecture belongs in the package name. It
should not. The index should contain one logical package/version record with
an artifact matrix keyed by at least:

- target architecture and machine family;
- EuryOS ABI revision;
- runtime/profile requirements;
- source revision;
- artifact digest;
- release signature and signing identity.

This is analogous to an apt package having one source/version identity and
separate binary packages for `arm64`, `amd64`, and so on, but the EuryOS
artifact key must also include the OS ABI and any profile-specific runtime
contract. A Pi 5 build and a QEMU build should be variants of `webserv`, not
different user-facing packages.

The eventual index entry should point to immutable release metadata, not a
mutable branch or latest URL. Index CI must reject an entry that omits a
supported-target artifact, capability declaration, digest, or signature.
