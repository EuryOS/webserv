# webserv

Native EuryOS web server.

This repository is the first standalone package-repository pilot for EuryOS.
The package name is deliberately just `webserv`; users install it with:

```text
pkg install webserv
```

The first release target is a small static-file server with explicit EuryOS
network and filesystem capabilities. It is not a POSIX port and does not
assume a global filesystem, root, environment, or process namespace.

## Repository shape

- `package/package.toml` — package identity and capability contract.
- `package/release.toml` — release artifact and target matrix proposal.
- `src/` — bounded, host-testable HTTP protocol core.
- `service/` — the `no_std` EuryOS service binary using the client-only
  `eury-sdk` package boundary.
- `docs/` — package and index integration notes.

The source repository is not itself the package index. The future index will
refer to a tagged source revision and a signed, content-addressed release
artifact.

## Current status

The first implementation slice is a bounded, `no_std`-compatible HTTP/1.1
protocol core in `webserv-core`. It supports GET and HEAD request parsing,
bounded headers, query separation, path-traversal rejection, and response
framing. It owns no sockets or filesystem handles.

The package metadata is intentionally explicit about the target/artifact
question that this pilot is meant to settle:

- one logical package name and SemVer version;
- one or more target-specific artifacts per release;
- target, ABI, profile, digest, and signature bound to each artifact;
- capabilities declared once in the package contract.

The service slice wires the core to EuryOS network and filesystem session
capabilities. Its `eury-sdk` Git revision and compatibility version are pinned
in `service/Cargo.toml` and the package metadata. The checked-in
`sdk/eury-sdk-bundle/0.1.0` is the standalone target/toolchain/linker input for
this example; it does not require an EuryOS source checkout. Build it with:

```sh
sh sdk/eury-sdk-bundle/0.1.0/build.sh service
```

The system repository's `make build-webserv` target remains the image-packaging
bridge until the package index can build and sign artifacts directly.

## Release builds

The versioned release workflow validates pull requests and publishes a
development-signed package archive whenever the package version changes on
`main`. It can also be started manually, or by pushing a tag named
`webserv-v<version>`.

Each release contains the installable `.eury` package archive, the standalone
ELF, SHA-256 checksums, and materialized release metadata. The current archive
uses EuryOS's public development trust anchor so it can be installed by the
development QEMU image. Production publication still needs a production
signing key and corresponding platform trust configuration.

## License

Proprietary. See the EuryOS project terms.
