/// A2UI protocol support for OSCortex.
///
/// Maps Google's Agent-to-UI (A2UI) protocol components to native
/// OSCortex design system widgets. Agents send A2UI JSON → the renderer
/// produces native OS-themed UI.
///
/// ## Quick Start
///
/// ```dart
/// import 'package:oscortex_ui/oscortex_ui.dart';
///
/// // Render an agent-composed UI:
/// A2UISurfaceWidget(
///   json: agentJsonPayload,
///   onAction: (action) {
///     agent.dispatch(action.name, action.params);
///   },
/// );
/// ```
///
/// ## Architecture
///
/// ```
/// Agent JSON (A2UI v0.8/v0.9)
///   → A2UIParser.parse()
///     → A2UISurface { components, data }
///       → A2UIRenderer
///         → OscButton, OscCard, OscTextField, ...
/// ```
///
/// ## Supported A2UI Components
///
/// | A2UI Component     | Maps To                        |
/// |--------------------|---------------------------------|
/// | Row                | Flutter Row                     |
/// | Column             | Flutter Column                  |
/// | List               | Flutter ListView                |
/// | Text               | Text + OscTypography            |
/// | Image              | ClipRRect + Image.network       |
/// | Icon               | Icon via A2UIIcons mapping      |
/// | Divider            | Container (1px)                 |
/// | Button             | OscButton / OscOutlineButton    |
/// | TextField          | OscTextField                    |
/// | CheckBox           | Checkbox + Row                  |
/// | Slider             | Slider (themed)                 |
/// | DateTimeInput      | OscButton → showDatePicker      |
/// | MultipleChoice     | OscChip wrap                    |
/// | Card               | OscCard                         |
/// | Modal              | Dialog                          |
/// | Tabs               | TabBar + TabBarView             |
library;

export 'a2ui_data_store.dart';
export 'a2ui_icons.dart';
export 'a2ui_parser.dart';
export 'a2ui_renderer.dart';
export 'a2ui_surface.dart';
export 'a2ui_types.dart';
