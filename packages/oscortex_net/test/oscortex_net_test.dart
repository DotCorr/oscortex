import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:oscortex_net/oscortex_net.dart';

/// Verifies the `oscortex/net` wire protocol from the Dart side: every request
/// frame OscortexSocket emits must match what the embedder's handle_net_channel
/// parses, and replies must be decoded correctly. The kernel/embedder is mocked
/// via the binary messenger, so these run without a device.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const String channelName = 'oscortex/net';

  final List<Uint8List> sent = <Uint8List>[];
  // Tunable canned recv reply (overridden per test).
  late int recvN;
  late List<int> recvData;

  Uint8List bytesOf(ByteData? m) =>
      m!.buffer.asUint8List(m.offsetInBytes, m.lengthInBytes);
  ByteData i32(int v) => ByteData(4)..setInt32(0, v, Endian.little);

  TestDefaultBinaryMessenger messenger() =>
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  setUp(() {
    sent.clear();
    recvN = 2;
    recvData = 'hi'.codeUnits;
    messenger().setMockMessageHandler(channelName, (ByteData? message) async {
      final Uint8List f = bytesOf(message);
      sent.add(f);
      switch (f[0]) {
        case 0x01: // connect → fd 3
          return i32(3);
        case 0x02: // status → established
          return i32(1);
        case 0x03: // send → bytes written == payload length
          return i32(f.length - 5);
        case 0x04: // recv → [i32 n | data]
          final int body = recvN > 0 ? recvData.length : 0;
          final ByteData r = ByteData(4 + body);
          r.setInt32(0, recvN, Endian.little);
          for (int i = 0; i < body; i++) {
            r.setUint8(4 + i, recvData[i]);
          }
          return r;
        case 0x05: // close → 0
          return i32(0);
        default:
          return i32(-22);
      }
    });
  });

  tearDown(() => messenger().setMockMessageHandler(channelName, null));

  test('connect sends [op, ip big-endian, port big-endian] and yields the fd',
      () async {
    final OscortexSocket sock = await OscortexSocket.connect('10.0.2.2', 8099);
    expect(sock.fd, 3);

    final Uint8List c = sent.first;
    expect(c[0], 0x01);
    expect(<int>[c[1], c[2], c[3], c[4]], <int>[10, 0, 2, 2]); // ip BE
    expect(<int>[c[5], c[6]], <int>[(8099 >> 8) & 0xFF, 8099 & 0xFF]); // port BE
    // connect must poll status (op 0x02) until established before returning.
    expect(sent.any((Uint8List f) => f[0] == 0x02), isTrue);
  });

  test('connect throws on a malformed IP (before sending anything)', () async {
    await expectLater(
      OscortexSocket.connect('not.an.ip', 80),
      throwsA(isA<OscortexNetException>()),
    );
    expect(sent, isEmpty);
  });

  test('connect throws when the kernel returns a negative fd', () async {
    messenger().setMockMessageHandler(channelName, (ByteData? m) async => i32(-23));
    await expectLater(
      OscortexSocket.connect('10.0.2.2', 80),
      throwsA(isA<OscortexNetException>()),
    );
  });

  test('write sends [op, fd little-endian, data] and returns bytes written',
      () async {
    final OscortexSocket sock = await OscortexSocket.connect('10.0.2.2', 80);
    sent.clear();

    final int n = await sock.write(<int>[1, 2, 3, 4]);
    expect(n, 4);

    final Uint8List w = sent.single;
    expect(w[0], 0x03);
    expect(<int>[w[1], w[2], w[3], w[4]], <int>[3, 0, 0, 0]); // fd=3 LE
    expect(w.sublist(5), <int>[1, 2, 3, 4]);
  });

  test('read sends [op, fd LE, max LE] and returns the payload bytes', () async {
    final OscortexSocket sock = await OscortexSocket.connect('10.0.2.2', 80);
    sent.clear();
    recvN = 5;
    recvData = 'hello'.codeUnits;

    final Uint8List data = await sock.read(max: 4096);
    expect(String.fromCharCodes(data), 'hello');

    final Uint8List r = sent.first;
    expect(r[0], 0x04);
    expect(<int>[r[1], r[2], r[3], r[4]], <int>[3, 0, 0, 0]); // fd LE
    final int maxField = r[5] | (r[6] << 8) | (r[7] << 16) | (r[8] << 24);
    expect(maxField, 4096); // max, little-endian
  });

  test('read returns empty once the timeout elapses with no data', () async {
    final OscortexSocket sock = await OscortexSocket.connect('10.0.2.2', 80);
    recvN = 0; // kernel: nothing available this poll
    final Uint8List data =
        await sock.read(timeout: const Duration(milliseconds: 80));
    expect(data, isEmpty);
  });

  test('close sends [op, fd little-endian]', () async {
    final OscortexSocket sock = await OscortexSocket.connect('10.0.2.2', 80);
    sent.clear();
    await sock.close();

    final Uint8List f = sent.single;
    expect(f[0], 0x05);
    expect(<int>[f[1], f[2], f[3], f[4]], <int>[3, 0, 0, 0]);
  });
}
