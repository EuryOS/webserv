# EuryOS SDK bundle 0.1.0

This is the checked-in native package build bundle used by the `webserv`
example. It is a versioned build input, not a checkout of EuryOS core code.

The bundle contains the pinned nightly toolchain, AArch64 package target
template, linker layout, `-Zbuild-std` settings, and the wrapper that resolves
the linker script to an absolute path before invoking Cargo.

The package manifest, signing/index tooling, and QEMU development image remain
system-distribution inputs. They are not silently implied by a crates.io crate.

Build the service from this repository with:

```sh
sh sdk/eury-sdk-bundle/0.1.0/build.sh service
```

The project must contain a `Cargo.lock`. The binary lands at
`target/aarch64-euryos-driver/release/webserv`.
