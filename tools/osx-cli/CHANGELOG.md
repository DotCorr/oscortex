# Changelog

## 0.1.0

Initial release.

- `osx init` — fetch the prebuilt OSCortex engine + gen_snapshot for both
  architectures and record per-app config.
- `osx build` — compile a Flutter app into a universal `.osx` bundle (native
  aarch64 + x86_64) plus per-arch bundles.
- `osx run` / `osx preview` — boot OSCortex in QEMU, with hardware acceleration
  for the host arch and emulation for the other.
