// Minimal dart:ui-only app — avoids Flutter framework JIT overhead on QEMU TCG.
// The entire kernel_blob.bin drops from ~41 MB to ~3 MB, making JIT startup
// fast enough to render within the QEMU timeout.
import 'dart:ui' as ui;

// One-shot flag so toImage test only runs on frame 1.
bool _toImageDone = false;

void main() {
  ui.PlatformDispatcher.instance.onDrawFrame = _drawFrame;
  ui.PlatformDispatcher.instance.scheduleFrame();
}

void _drawFrame() {
  // Always schedule the next frame first so the vsync loop never stops,
  // even if drawing throws an exception on this frame.
  ui.PlatformDispatcher.instance.scheduleFrame();

  final view = ui.PlatformDispatcher.instance.implicitView;
  if (view == null) {
    // ignore: avoid_print
    print('[dart] _drawFrame: implicitView is null');
    return;
  }

  try {
    // ignore: avoid_print
    print('[dart] _drawFrame: view ok size=${view.physicalSize}');

    // ── Diagnostic: toImage test on first frame ───────────────────────────
    // Build a tiny 4×4 picture, rasterize it offline, and dump the first pixel.
    // If b0==255 Skia is functioning; if 0 the JIT/Skia binding is broken.
    if (!_toImageDone) {
      _toImageDone = true;
      final trec = ui.PictureRecorder();
      final tc = ui.Canvas(trec, const ui.Rect.fromLTWH(0, 0, 4, 4));
      tc.drawColor(const ui.Color(0xFFFF0000), ui.BlendMode.src); // pure red
      final tpic = trec.endRecording();
      tpic.toImage(4, 4).then((img) {
        img.toByteData(format: ui.ImageByteFormat.rawRgba).then((bd) {
          if (bd != null && bd.lengthInBytes >= 4) {
            final b0 = bd.getUint8(0);
            final b1 = bd.getUint8(1);
            final b2 = bd.getUint8(2);
            final b3 = bd.getUint8(3);
            // ignore: avoid_print
            print('[dart] toImage b0=$b0 b1=$b1 b2=$b2 b3=$b3 (expect R=255 G=0 B=0 A=255 for red RGBA)');
          } else {
            // ignore: avoid_print
            print('[dart] toImage: toByteData null or empty');
          }
          img.dispose();
          tpic.dispose();
        });
      }).catchError((Object e) {
        // ignore: avoid_print
        print('[dart] toImage error: $e');
        tpic.dispose();
      });
    }

    // ── Main frame rendering ──────────────────────────────────────────────
    final rec = ui.PictureRecorder();
    // Use Rect.largest so no cull-rect mismatch can discard drawing commands.
    final canvas = ui.Canvas(rec, ui.Rect.largest);
    // White fill — every pixel should become 0xFF after present_callback sees it.
    canvas.drawColor(const ui.Color(0xFFFFFFFF), ui.BlendMode.src);
    // Red rectangle centred in 1280×800
    canvas.drawRect(
      const ui.Rect.fromLTWH(440, 300, 400, 200),
      ui.Paint()..color = const ui.Color(0xFFFF0000),
    );
    final pic = rec.endRecording();
    // ignore: avoid_print
    print('[dart] picture.approximateBytesUsed=${pic.approximateBytesUsed}');
    final builder = ui.SceneBuilder();
    // pushOffset is required by some engine versions to activate compositing.
    builder.pushOffset(0, 0);
    builder.addPicture(ui.Offset.zero, pic);
    builder.pop();
    final scene = builder.build();
    // ignore: avoid_print
    print('[dart] calling view.render');
    view.render(scene);
    scene.dispose();
    pic.dispose();
    // ignore: avoid_print
    print('[dart] view.render done');
  } catch (e, st) {
    // ignore: avoid_print
    print('[dart] _drawFrame exception: $e\n$st');
  }
}

