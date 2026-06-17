/// OSCortex app networking.
///
/// A thin TCP socket API for userspace Flutter apps, plus a minimal plaintext
/// HTTP client built on top of it. Everything funnels through the `oscortex/net`
/// platform channel (a `BasicMessageChannel<ByteData>` with the binary codec, so
/// TCP byte streams pass through unmangled) into the kernel TCP syscalls.
///
/// The kernel `tcp_connect` is asynchronous, so [OscortexSocket.connect] polls
/// the connection status until the handshake completes before returning.
///
/// Not yet supported (follow-up increments): DNS by hostname (pass an IP for
/// now) and TLS (so HTTP only, not HTTPS).
library oscortex_net;

import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/services.dart';

/// Raised when a networking operation fails; [code] is the kernel errno
/// (negative) or a client-side sentinel.
class OscortexNetException implements Exception {
  const OscortexNetException(this.message, this.code);
  final String message;
  final int code;
  @override
  String toString() => 'OscortexNetException($code): $message';
}

/// A connected TCP socket backed by a kernel socket fd.
class OscortexSocket {
  OscortexSocket._(this.fd);

  /// Kernel socket fd.
  final int fd;

  static const BasicMessageChannel<ByteData> _channel =
      BasicMessageChannel<ByteData>('oscortex/net', BinaryCodec());

  // Opcodes (must match the embedder's handle_net_channel).
  static const int _opConnect = 0x01;
  static const int _opStatus = 0x02;
  static const int _opSend = 0x03;
  static const int _opRecv = 0x04;
  static const int _opClose = 0x05;
  static const int _opResolve = 0x06;

  /// Resolve [host] to a dotted-quad IPv4 string, or null on failure.
  /// (DNS A-record lookup via the kernel resolver.)
  static Future<String?> resolve(String host) async {
    final List<int> name = host.codeUnits;
    final Uint8List req = Uint8List(1 + name.length)..[0] = _opResolve;
    req.setRange(1, 1 + name.length, name);
    final ByteData? reply = await _channel.send(ByteData.sublistView(req));
    if (reply == null || reply.lengthInBytes < 4) return null;
    // BE-order IPv4 as an unsigned u32; 0 = failure sentinel.
    final int v = reply.getUint32(0, Endian.little);
    if (v == 0) return null;
    return '${(v >> 24) & 0xFF}.${(v >> 16) & 0xFF}.${(v >> 8) & 0xFF}.${v & 0xFF}';
  }

  /// Send a request frame, return the leading little-endian i32 result.
  static Future<int> _callI32(Uint8List req) async {
    final ByteData? reply = await _channel.send(ByteData.sublistView(req));
    if (reply == null || reply.lengthInBytes < 4) return -22; // EINVAL
    return reply.getInt32(0, Endian.little);
  }

  static void _putFd(Uint8List req, int fd) {
    req[1] = fd & 0xFF;
    req[2] = (fd >> 8) & 0xFF;
    req[3] = (fd >> 16) & 0xFF;
    req[4] = (fd >> 24) & 0xFF;
  }

  static int _parseIpv4(String ip) {
    final List<String> parts = ip.split('.');
    if (parts.length != 4) {
      throw OscortexNetException('not an IPv4 address: $ip', -22);
    }
    var v = 0;
    for (final String p in parts) {
      final int o = int.tryParse(p) ?? -1;
      if (o < 0 || o > 255) {
        throw OscortexNetException('not an IPv4 address: $ip', -22);
      }
      v = (v << 8) | o;
    }
    return v & 0xFFFFFFFF;
  }

  /// Open a TCP connection to [ip] (dotted-quad) : [port], returning once the
  /// handshake has completed. Throws [OscortexNetException] on failure/timeout.
  static Future<OscortexSocket> connect(
    String ip,
    int port, {
    Duration timeout = const Duration(seconds: 10),
  }) async {
    final int ipBe = _parseIpv4(ip);
    final Uint8List req = Uint8List(7);
    req[0] = _opConnect;
    req[1] = (ipBe >> 24) & 0xFF;
    req[2] = (ipBe >> 16) & 0xFF;
    req[3] = (ipBe >> 8) & 0xFF;
    req[4] = ipBe & 0xFF;
    req[5] = (port >> 8) & 0xFF;
    req[6] = port & 0xFF;

    final int fd = await _callI32(req);
    if (fd < 0) throw OscortexNetException('connect failed', fd);

    final OscortexSocket sock = OscortexSocket._(fd);
    final DateTime deadline = DateTime.now().add(timeout);
    while (true) {
      final int st = await sock._status();
      if (st == 1) return sock; // established
      if (st < 0) {
        await sock.close();
        throw OscortexNetException('connection refused/reset', st);
      }
      if (DateTime.now().isAfter(deadline)) {
        await sock.close();
        throw const OscortexNetException('connect timed out', -110);
      }
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  Future<int> _status() {
    final Uint8List req = Uint8List(5)..[0] = _opStatus;
    _putFd(req, fd);
    return _callI32(req);
  }

  /// Write [data], retrying EAGAIN internally. Returns bytes written (which may
  /// be fewer than `data.length` — use [writeAll] to send everything).
  Future<int> write(
    List<int> data, {
    Duration timeout = const Duration(seconds: 5),
  }) async {
    final Uint8List req = Uint8List(5 + data.length);
    req[0] = _opSend;
    _putFd(req, fd);
    req.setRange(5, 5 + data.length, data);

    final DateTime deadline = DateTime.now().add(timeout);
    while (true) {
      final int n = await _callI32(req);
      if (n != -11) {
        // not EAGAIN
        if (n < 0) throw OscortexNetException('write failed', n);
        return n;
      }
      if (DateTime.now().isAfter(deadline)) {
        throw const OscortexNetException('write timed out', -110);
      }
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
  }

  /// Write every byte of [data], looping until the buffer is drained.
  Future<void> writeAll(List<int> data) async {
    var off = 0;
    while (off < data.length) {
      off += await write(data.sublist(off));
    }
  }

  /// Read up to [max] bytes. Returns the bytes received, or an empty list if no
  /// data arrived within [timeout] (e.g. the peer closed or is idle). The caller
  /// loops until it has what it needs (see [httpGet]).
  Future<Uint8List> read({
    int max = 8192,
    Duration timeout = const Duration(seconds: 2),
  }) async {
    final Uint8List req = Uint8List(9)..[0] = _opRecv;
    _putFd(req, fd);
    req.buffer.asByteData().setUint32(5, max, Endian.little);

    final DateTime deadline = DateTime.now().add(timeout);
    while (true) {
      final ByteData? reply = await _channel.send(ByteData.sublistView(req));
      if (reply == null || reply.lengthInBytes < 4) {
        throw const OscortexNetException('read failed', -5);
      }
      final int n = reply.getInt32(0, Endian.little);
      if (n > 0) {
        // Copy out of the channel buffer (don't alias it).
        return Uint8List.fromList(
          reply.buffer.asUint8List(reply.offsetInBytes + 4, n),
        );
      }
      if (n < 0 && n != -11) {
        throw OscortexNetException('read failed', n);
      }
      // n == 0 (nothing this poll) or -11 (EAGAIN): wait and retry until deadline.
      if (DateTime.now().isAfter(deadline)) return Uint8List(0);
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  /// Close the socket.
  Future<void> close() async {
    final Uint8List req = Uint8List(5)..[0] = _opClose;
    _putFd(req, fd);
    await _callI32(req);
  }
}

/// Minimal plaintext HTTP/1.0 GET over a raw TCP socket (no TLS yet, so HTTP
/// only). [ip] is the server's dotted-quad address, [host] the Host header,
/// [path] the request path. Returns the response body (headers stripped).
///
/// Until DNS lands, resolve the host to an IP out-of-band and pass it here.
Future<String> httpGet(
  String ip,
  String host,
  String path, {
  int port = 80,
}) async {
  final OscortexSocket sock = await OscortexSocket.connect(ip, port);
  try {
    final String request =
        'GET $path HTTP/1.0\r\nHost: $host\r\nConnection: close\r\n\r\n';
    await sock.writeAll(request.codeUnits);

    final BytesBuilder body = BytesBuilder();
    while (true) {
      final Uint8List chunk = await sock.read();
      if (chunk.isEmpty) break; // peer closed / idle past the read timeout
      body.add(chunk);
    }

    final String text = String.fromCharCodes(body.takeBytes());
    final int sep = text.indexOf('\r\n\r\n');
    return sep >= 0 ? text.substring(sep + 4) : text;
  } finally {
    await sock.close();
  }
}
