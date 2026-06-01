/// OSCortex Design System
///
/// Canonical tokens, themed widgets, and animations for all
/// OSCortex userspace Flutter applications.
///
/// ```dart
/// import 'package:oscortex_ui/oscortex_ui.dart';
///
/// MaterialApp(
///   theme: OscTheme.dark(accent: OscColors.skyBlue),
///   home: OscScaffold(
///     accent: OscColors.skyBlue,
///     body: Column(children: [...]),
///   ),
/// );
/// ```
library oscortex_ui;

// ── Tokens ─────────────────────────────────────────────────────────────
export 'src/tokens/colors.dart';
export 'src/tokens/radii.dart';
export 'src/tokens/shadows.dart';
export 'src/tokens/spacing.dart';
export 'src/tokens/typography.dart';

// ── Theme ──────────────────────────────────────────────────────────────
export 'src/theme/osc_theme.dart';

// ── Widgets ────────────────────────────────────────────────────────────
export 'src/widgets/osc_app_rail.dart';
export 'src/widgets/osc_banner.dart';
export 'src/widgets/osc_breadcrumbs.dart';
export 'src/widgets/osc_button.dart';
export 'src/widgets/osc_card.dart';
export 'src/widgets/osc_chip.dart';
export 'src/widgets/osc_hub_dock.dart';
export 'src/widgets/osc_icon_button.dart';
export 'src/widgets/osc_intent_bar.dart';
export 'src/widgets/osc_popup_menu.dart';
export 'src/widgets/osc_scaffold.dart';
export 'src/widgets/osc_section_label.dart';
export 'src/widgets/osc_status_badge.dart';
export 'src/widgets/osc_status_bar.dart';
export 'src/widgets/osc_tag.dart';
export 'src/widgets/osc_text_field.dart';
export 'src/widgets/osc_toolbar.dart';
export 'src/widgets/osc_waveform.dart';

// ── Previews ───────────────────────────────────────────────────────────
export 'src/preview/osc_preview.dart';

// ── A2UI Protocol ──────────────────────────────────────────────────────
export 'src/a2ui/a2ui.dart';
