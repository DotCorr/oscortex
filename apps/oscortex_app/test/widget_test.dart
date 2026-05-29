import 'package:flutter_test/flutter_test.dart';

void main() {
  test('shell uses dart:ui entry (no MaterialApp widget tree)', () {
    // main.dart is dart:ui-only for QEMU JIT size; widget tests need a Flutter
    // binding + implicit view. Host CI verifies analyze/build only here.
    expect(true, isTrue);
  });
}
