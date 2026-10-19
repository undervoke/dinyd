# Distribution

[dist configuration](../dist-workspace.toml) owns the target and artifact inventory; the [release workflow](../.github/workflows/release.yml) is generated from it.
`rust-version` in [Cargo.toml](../Cargo.toml), the channel in [rust-toolchain.toml](../rust-toolchain.toml), and the minimum Rust in the [README](../README.md#install) name one version; change them together.
Native archives and bootstrap installers support [project setup](installation.md) without a consumer build toolchain.
The manual archive-and-checksum path remains available because stock bootstrap installers do not guarantee checksum verification on every platform.
Version-cache downloads and self-update share native acquisition with full SHA-256 verification against the release sidecar.
A cache executable becomes visible only when complete, because several hooks can start concurrently.
Executable replacement must support Windows, where the running executable remains in use.
[Installation](installation.md) owns cache identity and update destinations.

@import "distribution.mermaid"
