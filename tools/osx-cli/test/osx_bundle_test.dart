import 'dart:io';
import 'dart:typed_data';

import 'package:oscortex_cli/src/osx_bundle.dart';
import 'package:oscortex_cli/src/util/host.dart';
import 'package:path/path.dart' as p;
import 'package:test/test.dart';

/// Reads the v1 header fields the kernel parser (app_registry/mod.rs) reads.
({String magic, int version, String name, String ver, int aotLen}) _readHeader(
    Uint8List b) {
  final bd = ByteData.sublistView(b);
  String cstr(int start, int len) {
    final slice = b.sublist(start, start + len);
    final end = slice.indexOf(0);
    return String.fromCharCodes(slice.sublist(0, end == -1 ? len : end));
  }

  return (
    magic: String.fromCharCodes(b.sublist(0, 4)),
    version: bd.getUint32(4, Endian.little),
    name: cstr(8, 64),
    ver: cstr(72, 16),
    aotLen: bd.getUint32(104, Endian.little),
  );
}

void main() {
  Uint8List fakeSnap(int len, int fill) =>
      Uint8List.fromList(List.filled(len, fill));

  group('single-arch v1 bundle', () {
    test('header matches the kernel/Python v1 layout', () {
      final tmp = Directory.systemTemp.createTempSync('osx_test');
      final out = File(p.join(tmp.path, 'App.osx'));
      final snap = fakeSnap(2048, 0xAB);

      OsxBundle.writeSingleArch(
        snapshot: snap,
        name: 'Demo',
        version: '1.2.3',
        out: out,
      );

      final bytes = out.readAsBytesSync();
      final h = _readHeader(bytes);

      expect(h.magic, 'OSCP');
      expect(h.version, 1);
      expect(h.name, 'Demo');
      expect(h.ver, '1.2.3');
      expect(h.aotLen, 2048);
      // Payload starts at byte 112 and equals the snapshot.
      expect(bytes.length, OsxBundle.headerSize + 2048);
      expect(bytes.sublist(OsxBundle.headerSize), snap);

      tmp.deleteSync(recursive: true);
    });
  });

  group('universal (fat) bundle', () {
    test('is v1-readable: header describes the primary arch snapshot', () {
      final tmp = Directory.systemTemp.createTempSync('osx_test');
      final out = File(p.join(tmp.path, 'App.osx'));
      final arm = fakeSnap(1000, 0x11);
      final x64 = fakeSnap(1500, 0x22);

      OsxBundle.writeUniversal(
        snapshots: {Arch.aarch64: arm, Arch.x86_64: x64},
        primary: Arch.aarch64,
        name: 'Demo',
        version: '0.1.0',
        out: out,
      );

      final bytes = out.readAsBytesSync();
      final h = _readHeader(bytes);

      // An UNMODIFIED kernel reads the v1 header + the first aot_len bytes.
      expect(h.magic, 'OSCP');
      expect(h.version, 1);
      expect(h.aotLen, arm.length, reason: 'primary snapshot in the v1 slot');
      expect(
        bytes.sublist(OsxBundle.headerSize, OsxBundle.headerSize + arm.length),
        arm,
        reason: 'v1 loader gets the primary arch verbatim',
      );
    });

    test('OSXM index lists both arches with recoverable snapshots', () {
      final tmp = Directory.systemTemp.createTempSync('osx_test');
      final out = File(p.join(tmp.path, 'App.osx'));
      final arm = fakeSnap(1000, 0x11);
      final x64 = fakeSnap(1500, 0x22);

      OsxBundle.writeUniversal(
        snapshots: {Arch.aarch64: arm, Arch.x86_64: x64},
        primary: Arch.x86_64,
        name: 'Demo',
        version: '0.1.0',
        out: out,
      );

      final bytes = out.readAsBytesSync();
      final entries = _parseIndex(bytes);

      expect(entries.keys, containsAll(['arm64', 'x64']));

      // Each recorded (offset,len) must point at the right snapshot bytes.
      final armRec = entries['arm64']!;
      final x64Rec = entries['x64']!;
      expect(bytes.sublist(armRec.$1, armRec.$1 + armRec.$2), arm);
      expect(bytes.sublist(x64Rec.$1, x64Rec.$1 + x64Rec.$2), x64);

      // The x64 (primary) snapshot must also be the v1 payload.
      expect(x64Rec.$1, OsxBundle.headerSize);

      tmp.deleteSync(recursive: true);
    });
  });
}

/// Scan for the OSXM index and decode its records.
Map<String, (int, int)> _parseIndex(Uint8List b) {
  // Locate "OSXM" after the header.
  final magic = [0x4F, 0x53, 0x58, 0x4D];
  var idx = -1;
  for (var i = OsxBundle.headerSize; i < b.length - 4; i++) {
    if (b[i] == magic[0] &&
        b[i + 1] == magic[1] &&
        b[i + 2] == magic[2] &&
        b[i + 3] == magic[3]) {
      idx = i;
      break;
    }
  }
  expect(idx, isNot(-1), reason: 'OSXM index present');

  final bd = ByteData.sublistView(b);
  final count = bd.getUint32(idx + 8, Endian.little);
  final out = <String, (int, int)>{};
  var o = idx + 16;
  for (var i = 0; i < count; i++) {
    final tokBytes = b.sublist(o, o + 8);
    final end = tokBytes.indexOf(0);
    final tok = String.fromCharCodes(tokBytes.sublist(0, end == -1 ? 8 : end));
    final off = bd.getUint64(o + 8, Endian.little);
    final len = bd.getUint64(o + 16, Endian.little);
    out[tok] = (off, len);
    o += 24;
  }
  return out;
}
