#include <stdio.h>
#include <stdint.h>
#include <stddef.h>
#include "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/flutter_embedder.h"

int main() {
    printf("sizeof(FlutterPointerEvent) = %zu\n", sizeof(FlutterPointerEvent));
    printf("struct_size = %zu\n", offsetof(FlutterPointerEvent, struct_size));
    printf("phase = %zu\n", offsetof(FlutterPointerEvent, phase));
    printf("timestamp = %zu\n", offsetof(FlutterPointerEvent, timestamp));
    printf("x = %zu\n", offsetof(FlutterPointerEvent, x));
    printf("y = %zu\n", offsetof(FlutterPointerEvent, y));
    printf("device = %zu\n", offsetof(FlutterPointerEvent, device));
    printf("signal_kind = %zu\n", offsetof(FlutterPointerEvent, signal_kind));
    printf("scroll_delta_x = %zu\n", offsetof(FlutterPointerEvent, scroll_delta_x));
    printf("scroll_delta_y = %zu\n", offsetof(FlutterPointerEvent, scroll_delta_y));
    printf("device_kind = %zu\n", offsetof(FlutterPointerEvent, device_kind));
    printf("buttons = %zu\n", offsetof(FlutterPointerEvent, buttons));
    printf("pan_x = %zu\n", offsetof(FlutterPointerEvent, pan_x));
    printf("pan_y = %zu\n", offsetof(FlutterPointerEvent, pan_y));
    printf("scale = %zu\n", offsetof(FlutterPointerEvent, scale));
    printf("rotation = %zu\n", offsetof(FlutterPointerEvent, rotation));
    printf("view_id = %zu\n", offsetof(FlutterPointerEvent, view_id));
    return 0;
}
