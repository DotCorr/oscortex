# oscortex_net

App networking for OSCortex userspace Flutter apps — a TCP socket plus a minimal
plaintext HTTP client, over the `oscortex/net` platform channel into the kernel
TCP stack.

## Usage

```dart
import 'package:oscortex_net/oscortex_net.dart';

// Raw TCP
final sock = await OscortexSocket.connect('10.0.2.2', 8080);
await sock.writeAll('ping'.codeUnits);
final reply = await sock.read();
await sock.close();

// Minimal HTTP GET (plaintext / port 80; pass the server IP for now)
final body = await httpGet('93.184.216.34', 'example.com', '/');
```

`OscortexSocket.connect` returns only once the (asynchronous) kernel handshake
has completed, so you can write immediately.

## Status

| Capability | State |
|---|---|
| TCP connect / status / send / recv / close | ✅ (kernel syscalls + embedder channel) |
| Plaintext HTTP/1.0 GET | ✅ helper |
| DNS by hostname | ⏳ next increment (pass an IP for now) |
| TLS / HTTPS | ⏳ (smoltcp has no TLS; needs a userspace TLS layer) |

## How it works

```
Dart (this package)
  └─ BasicMessageChannel<ByteData>('oscortex/net', BinaryCodec())   compact opcode frames
       └─ embedder handle_net_channel  (tools/flutter-embedder/src/main.rs)
            └─ TCP syscalls 0x388-0x38B + tcp_status 0x4B7  (CAP_NET)
                 └─ net::tcp (smoltcp)  — the same stack the package pipeline uses
```

Apps are granted `CAP_NET` by the kernel so these calls are permitted (the
security phase will make that a per-app declared/prompted permission).
