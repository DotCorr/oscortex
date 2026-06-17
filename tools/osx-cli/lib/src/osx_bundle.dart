import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:path/path.dart' as p;

import 'util/host.dart';

/// Writes OSCortex `.osx` bundles.
///
/// ## Background
/// The kernel's app registry (`kernel/src/app_registry/mod.rs`) reads a v1
/// bundle: a fixed 112-byte header (`OSCP` magic, `format_version=1`, name,
/// version, entry_offset, stack_size, `aot_len`) followed by exactly one AOT
/// snapshot. It copies `aot_len` bytes into the app record and **ignores
/// everything past `112 + aot_len`**. There is no per-arch selection in v1 — a
/// v1 bundle is implicitly single-arch.
///
/// ## What this packer produces
/// We want app developers to ship ONE artifact that is native for both
/// aarch64 and x86_64. Two outputs are produced from one build:
///
/// 1. **Per-arch v1 bundles** (`<App>-arm64.osx`, `<App>-x64.osx`) — each is a
///    plain v1 bundle the current kernel installs and runs unchanged. These are
///    what actually boot today.
///
/// 2. **A universal (fat) bundle** (`<App>.osx`) — a strict superset:
///      * Bytes `0..112`  : a **valid v1 header** whose `aot_len` covers the
///        host-arch snapshot, placed first. An unmodified kernel reads this and
///        loads that arch — so the fat bundle never bricks an old OS.
///      * `112 .. 112+aot_len` : the primary (host-arch) AOT snapshot.
///      * **trailer** (ignored by v1): a `OSXM` multi-arch index listing every
///        arch's (token, offset, length) so an arch-aware loader can pick the
///        right snapshot at install/run time, followed by the non-primary
///        snapshots.
///
/// The trailer is documented below so the kernel can gain arch-selection later
/// without changing the v1 path. Until then the per-arch bundles are the
/// reliable install targets and the maintainer report flags this.
class OsxBundle {
  // v1 header layout (must match oscortex-pack.py + the kernel parser).
  static const _magic = [0x4F, 0x53, 0x43, 0x50]; // "OSCP"
  static const _formatVersion = 1;
  static const headerSize = 112;

  // Trailer magic for the multi-arch index: "OSXM".
  static const _trailerMagic = [0x4F, 0x53, 0x58, 0x4D];
  static const _trailerVersion = 1;

  /// Build a single-arch v1 bundle for [snapshot] and write it to [out].
  static void writeSingleArch({
    required Uint8List snapshot,
    required String name,
    required String version,
    required File out,
  }) {
    final header = _v1Header(
      name: name,
      version: version,
      aotLen: snapshot.length,
    );
    out.parent.createSync(recursive: true);
    final sink = out.openSync(mode: FileMode.write);
    try {
      sink.writeFromSync(header);
      sink.writeFromSync(snapshot);
    } finally {
      sink.closeSync();
    }
  }

  /// Build the universal (fat) bundle from per-arch [snapshots], with
  /// [primary] placed in the v1 slot (so an unmodified kernel boots it).
  ///
  /// Layout: [v1 header][primary snapshot][OSXM index][other snapshots...].
  static void writeUniversal({
    required Map<Arch, Uint8List> snapshots,
    required Arch primary,
    required String name,
    required String version,
    required File out,
  }) {
    assert(snapshots.containsKey(primary));
    final primarySnap = snapshots[primary]!;

    // Order: primary first (its bytes live right after the header so the v1
    // header's aot_len describes them), then the rest in a stable order.
    final ordered = <Arch>[
      primary,
      ...snapshots.keys.where((a) => a != primary)
    ];

    final builder = BytesBuilder(copy: false);
    builder.add(
        _v1Header(name: name, version: version, aotLen: primarySnap.length));

    // Compute offsets as we lay snapshots down. The index records absolute
    // file offsets so a loader can seek directly.
    final entries = <_ArchEntry>[];
    var cursor = headerSize;

    // Primary snapshot immediately follows the header.
    builder.add(primarySnap);
    entries.add(_ArchEntry(primary, cursor, primarySnap.length));
    cursor += primarySnap.length;

    // Reserve the index size so we can record post-index offsets correctly:
    // the index has a fixed-size head + one fixed record per arch.
    final indexLen = _indexLength(ordered.length);
    final indexOffset = cursor;
    cursor += indexLen;

    // Non-primary snapshots come after the index.
    final tail = BytesBuilder(copy: false);
    for (final a in ordered.where((a) => a != primary)) {
      final snap = snapshots[a]!;
      entries.add(_ArchEntry(a, cursor, snap.length));
      tail.add(snap);
      cursor += snap.length;
    }

    // Now we can build the index (offsets are all known) and append + tail.
    builder.add(_buildIndex(entries, indexOffset, indexLen));
    builder.add(tail.toBytes());

    out.parent.createSync(recursive: true);
    out.writeAsBytesSync(builder.toBytes());
  }

  static Uint8List _v1Header({
    required String name,
    required String version,
    required int aotLen,
    int entryOffset = 0,
    int stackSize = 0,
  }) {
    final h = Uint8List(headerSize);
    final bd = ByteData.sublistView(h);
    h.setRange(0, 4, _magic);
    bd.setUint32(4, _formatVersion, Endian.little);

    final nameB = _utf8Clamp(name, 63);
    h.setRange(8, 8 + nameB.length, nameB);
    final verB = _utf8Clamp(version, 15);
    h.setRange(72, 72 + verB.length, verB);

    bd.setUint64(88, entryOffset, Endian.little);
    bd.setUint64(96, stackSize, Endian.little);
    bd.setUint32(104, aotLen, Endian.little);
    bd.setUint32(108, 0, Endian.little); // reserved
    return h;
  }

  // OSXM index: [magic(4)][ver(u32)][count(u32)][reserved(u32)] then `count`
  // records of [token(8, null-padded ascii)][offset(u64)][len(u64)].
  static const _idxHead = 16;
  static const _idxRecord = 24;
  static int _indexLength(int count) => _idxHead + _idxRecord * count;

  static Uint8List _buildIndex(List<_ArchEntry> entries, int at, int len) {
    final b = Uint8List(len);
    final bd = ByteData.sublistView(b);
    b.setRange(0, 4, _trailerMagic);
    bd.setUint32(4, _trailerVersion, Endian.little);
    bd.setUint32(8, entries.length, Endian.little);
    bd.setUint32(12, 0, Endian.little);
    var o = _idxHead;
    for (final e in entries) {
      final tok = _utf8Clamp(e.arch.osxToken, 8);
      b.setRange(o, o + tok.length, tok);
      bd.setUint64(o + 8, e.offset, Endian.little);
      bd.setUint64(o + 16, e.length, Endian.little);
      o += _idxRecord;
    }
    return b;
  }

  static Uint8List _utf8Clamp(String s, int maxBytes) {
    final bytes = utf8.encode(s);
    return bytes.length <= maxBytes
        ? bytes
        : Uint8List.sublistView(bytes, 0, maxBytes);
  }
}

/// Output paths for a build: the universal bundle plus one per arch, all in
/// [outDir] and named after [displayName].
class OsxOutputs {
  OsxOutputs(String outDir, String displayName)
      : universal = File(p.join(outDir, '$displayName.osx')),
        perArch = {
          for (final a in Arch.values)
            a: File(p.join(outDir, '$displayName-${a.osxToken}.osx')),
        };

  final File universal;
  final Map<Arch, File> perArch;
}

class _ArchEntry {
  _ArchEntry(this.arch, this.offset, this.length);
  final Arch arch;
  final int offset;
  final int length;
}
