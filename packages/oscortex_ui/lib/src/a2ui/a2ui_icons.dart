import 'package:flutter/material.dart';

/// Maps A2UI icon names to Material [IconData].
///
/// A2UI defines a basic icon set; this maps those names to
/// the closest Material Symbols equivalent.
class A2UIIcons {
  const A2UIIcons._();

  static const Map<String, IconData> _map = {
    // Navigation
    'home': Icons.home_rounded,
    'back': Icons.arrow_back_rounded,
    'forward': Icons.arrow_forward_rounded,
    'menu': Icons.menu_rounded,
    'close': Icons.close_rounded,
    'more': Icons.more_vert_rounded,
    'more_horiz': Icons.more_horiz_rounded,

    // Actions
    'check': Icons.check_rounded,
    'add': Icons.add_rounded,
    'remove': Icons.remove_rounded,
    'delete': Icons.delete_rounded,
    'edit': Icons.edit_rounded,
    'save': Icons.save_rounded,
    'copy': Icons.copy_rounded,
    'paste': Icons.paste_rounded,
    'undo': Icons.undo_rounded,
    'redo': Icons.redo_rounded,
    'search': Icons.search_rounded,
    'refresh': Icons.refresh_rounded,
    'download': Icons.download_rounded,
    'upload': Icons.upload_rounded,
    'share': Icons.share_rounded,
    'send': Icons.send_rounded,
    'link': Icons.link_rounded,
    'print': Icons.print_rounded,

    // Status
    'info': Icons.info_rounded,
    'warning': Icons.warning_rounded,
    'error': Icons.error_rounded,
    'success': Icons.check_circle_rounded,
    'help': Icons.help_rounded,
    'pending': Icons.pending_rounded,

    // Media
    'play': Icons.play_arrow_rounded,
    'pause': Icons.pause_rounded,
    'stop': Icons.stop_rounded,
    'skip_next': Icons.skip_next_rounded,
    'skip_previous': Icons.skip_previous_rounded,
    'volume_up': Icons.volume_up_rounded,
    'volume_down': Icons.volume_down_rounded,
    'volume_off': Icons.volume_off_rounded,
    'mic': Icons.mic_rounded,
    'camera': Icons.camera_alt_rounded,
    'image': Icons.image_rounded,
    'video': Icons.videocam_rounded,
    'music': Icons.music_note_rounded,

    // Files
    'file': Icons.insert_drive_file_rounded,
    'folder': Icons.folder_rounded,
    'folder_open': Icons.folder_open_rounded,
    'attachment': Icons.attach_file_rounded,
    'document': Icons.description_rounded,

    // Communication
    'email': Icons.email_rounded,
    'chat': Icons.chat_rounded,
    'notification': Icons.notifications_rounded,
    'phone': Icons.phone_rounded,

    // User
    'person': Icons.person_rounded,
    'group': Icons.group_rounded,
    'settings': Icons.settings_rounded,
    'lock': Icons.lock_rounded,
    'unlock': Icons.lock_open_rounded,
    'visibility': Icons.visibility_rounded,
    'visibility_off': Icons.visibility_off_rounded,

    // Misc
    'star': Icons.star_rounded,
    'favorite': Icons.favorite_rounded,
    'bookmark': Icons.bookmark_rounded,
    'calendar': Icons.calendar_today_rounded,
    'clock': Icons.access_time_rounded,
    'location': Icons.location_on_rounded,
    'map': Icons.map_rounded,
    'language': Icons.language_rounded,
    'code': Icons.code_rounded,
    'terminal': Icons.terminal_rounded,
    'dashboard': Icons.dashboard_rounded,
    'analytics': Icons.analytics_rounded,
    'cloud': Icons.cloud_rounded,
    'wifi': Icons.wifi_rounded,
    'bluetooth': Icons.bluetooth_rounded,
    'battery': Icons.battery_full_rounded,
    'light': Icons.light_mode_rounded,
    'dark': Icons.dark_mode_rounded,
  };

  /// Resolve an A2UI icon name to [IconData].
  /// Falls back to [Icons.circle] for unknown names.
  static IconData resolve(String name) {
    return _map[name.toLowerCase()] ?? Icons.circle;
  }

  /// Check if a name maps to a known icon.
  static bool isKnown(String name) => _map.containsKey(name.toLowerCase());
}
