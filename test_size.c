
#include <stdio.h>
#include <stdint.h>
#include <stddef.h>
#include "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/flutter_embedder.h"

int main() {
    printf("sizeof(FlutterWindowMetricsEvent) = %zu\n", sizeof(FlutterWindowMetricsEvent));
    printf("offsetof(struct_size) = %zu\n", offsetof(FlutterWindowMetricsEvent, struct_size));
    printf("offsetof(width) = %zu\n", offsetof(FlutterWindowMetricsEvent, width));
    printf("offsetof(height) = %zu\n", offsetof(FlutterWindowMetricsEvent, height));
    printf("offsetof(pixel_ratio) = %zu\n", offsetof(FlutterWindowMetricsEvent, pixel_ratio));
    printf("offsetof(left) = %zu\n", offsetof(FlutterWindowMetricsEvent, left));
    printf("offsetof(top) = %zu\n", offsetof(FlutterWindowMetricsEvent, top));
    printf("offsetof(physical_view_inset_top) = %zu\n", offsetof(FlutterWindowMetricsEvent, physical_view_inset_top));
    printf("offsetof(physical_view_inset_right) = %zu\n", offsetof(FlutterWindowMetricsEvent, physical_view_inset_right));
    printf("offsetof(physical_view_inset_bottom) = %zu\n", offsetof(FlutterWindowMetricsEvent, physical_view_inset_bottom));
    printf("offsetof(physical_view_inset_left) = %zu\n", offsetof(FlutterWindowMetricsEvent, physical_view_inset_left));
    printf("offsetof(display_id) = %zu\n", offsetof(FlutterWindowMetricsEvent, display_id));
    printf("offsetof(view_id) = %zu\n", offsetof(FlutterWindowMetricsEvent, view_id));

    printf("\nsizeof(FlutterEngineDisplay) = %zu\n", sizeof(FlutterEngineDisplay));
    printf("offsetof(display_id) = %zu\n", offsetof(FlutterEngineDisplay, display_id));
    printf("offsetof(single_display) = %zu\n", offsetof(FlutterEngineDisplay, single_display));
    printf("offsetof(refresh_rate) = %zu\n", offsetof(FlutterEngineDisplay, refresh_rate));
    printf("offsetof(width) = %zu\n", offsetof(FlutterEngineDisplay, width));
    printf("offsetof(height) = %zu\n", offsetof(FlutterEngineDisplay, height));
    printf("offsetof(device_pixel_ratio) = %zu\n", offsetof(FlutterEngineDisplay, device_pixel_ratio));
    return 0;
}
