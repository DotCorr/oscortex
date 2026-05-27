// Minimal dart:ui-only app — avoids Flutter framework JIT overhead on QEMU TCG.
import 'dart:ui' as ui;

void main() {
  ui.PlatformDispatcher.instance.onDrawFrame = _drawFrame;
  ui.PlatformDispatcher.instance.scheduleFrame();
}

void _drawFrame() {
  ui.PlatformDispatcher.instance.scheduleFrame();

  final view = ui.PlatformDispatcher.instance.implicitView;
  if (view == null) return;

  try {
    final rec = ui.PictureRecorder();
    final canvas = ui.Canvas(rec, ui.Rect.largest);
    canvas.drawColor(const ui.Color(0xFF0C1C26), ui.BlendMode.src);
    final pic = rec.endRecording();
    final builder = ui.SceneBuilder();
    builder.addPicture(ui.Offset.zero, pic);
    final scene = builder.build();
    view.render(scene);
    scene.dispose();
    pic.dispose();
  } catch (_) {
    // Keep the vsync loop alive even if rendering throws.
  }
}
