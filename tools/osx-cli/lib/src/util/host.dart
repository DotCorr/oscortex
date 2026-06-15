import 'dart:io';

/// CPU architectures OSCortex targets.
enum Arch {
  aarch64,
  x86_64;

  /// Short token used in artifact names: `arm64` / `x64`.
  /// Matches `engine-port/fetch-engine.sh` and the engine release assets.
  String get engineToken => this == Arch.aarch64 ? 'arm64' : 'x64';

  /// Token used in the .osx per-arch payload index and filenames.
  String get osxToken => this == Arch.aarch64 ? 'arm64' : 'x64';

  /// Human label.
  String get label => this == Arch.aarch64 ? 'aarch64' : 'x86_64';

  static Arch? parse(String s) {
    switch (s.toLowerCase()) {
      case 'arm64':
      case 'aarch64':
        return Arch.aarch64;
      case 'x64':
      case 'x86_64':
      case 'amd64':
        return Arch.x86_64;
    }
    return null;
  }
}

/// Facts about the machine the CLI is running on. Used to decide HVF/KVM vs TCG
/// and which gen_snapshot can run natively.
class Host {
  Host({required this.os, required this.arch});

  /// 'macos' | 'linux' | 'windows' | other (`Platform.operatingSystem`).
  final String os;

  /// Host CPU architecture, or null if unrecognised.
  final Arch? arch;

  static Host detect() {
    return Host(os: Platform.operatingSystem, arch: _detectArch());
  }

  bool get isMacOS => os == 'macos';
  bool get isLinux => os == 'linux';

  /// The hardware accelerator available for QEMU on this host, if any.
  /// `hvf` on Apple Silicon/Intel macOS, `kvm` on Linux, else null (TCG only).
  String? get accelerator {
    if (isMacOS) return 'hvf';
    if (isLinux && File('/dev/kvm').existsSync()) return 'kvm';
    return null;
  }

  /// True when [target] can run with hardware acceleration on this host —
  /// i.e. the requested arch matches the host arch and an accelerator exists.
  bool canAccelerate(Arch target) => arch == target && accelerator != null;

  static Arch? _detectArch() {
    // Platform.version embeds the host arch token, e.g. "... on \"macos_arm64\"".
    final v = Platform.version;
    if (v.contains('arm64') || v.contains('aarch64')) return Arch.aarch64;
    if (v.contains('x64') || v.contains('x86_64')) return Arch.x86_64;

    // Fallback: `uname -m` on POSIX.
    if (!Platform.isWindows) {
      try {
        final out = Process.runSync('uname', ['-m']).stdout as String;
        return Arch.parse(out.trim());
      } catch (_) {/* ignore */}
    }
    return null;
  }
}
