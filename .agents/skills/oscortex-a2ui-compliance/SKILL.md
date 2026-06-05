---
name: oscortex-a2ui-compliance
description: >
  Enforce A2UI protocol compliance for all OSCortex UI components.
  Trigger when: adding a new Osc* widget, creating a new interactive component,
  modifying the oscortex_ui package, or any question about A2UI support.
  Ensures every new component has an A2UI renderer mapping and is exported correctly.
---

# OSCortex A2UI Compliance

## Purpose

Every interactive or container component in the `oscortex_ui` package **MUST** have
a corresponding A2UI renderer mapping so that AI agents can compose UI declaratively.

## Rules

### 1. No Material UI in Interactive Components

OSCortex is its own OS — **never** use these Material widgets for interactive/container components:

| ❌ FORBIDDEN Material Widget | ✅ Use Instead |
|------------------------------|----------------|
| `Checkbox` | `OscCheckbox` |
| `Slider` | `OscSlider` |
| `showDatePicker()` | `OscDatePicker` |
| `Dialog` / `AlertDialog` | `OscModal` |
| `TabBar` / `TabBarView` | `OscTabs` |
| `DropdownButton` | `OscPopupMenu` |
| `Switch` | Build `OscSwitch` |
| `Radio` | Build `OscRadio` |
| `SnackBar` / `ScaffoldMessenger` | `OscBanner` |
| `BottomSheet` | Build `OscSheet` |
| `AppBar` | `OscToolbar` |
| `NavigationRail` | `OscAppRail` |
| `FloatingActionButton` | `OscButton` |
| `Chip` / `InputChip` / `FilterChip` | `OscChip` / `OscTag` |

**Passthrough OK** — these carry no design language and may be used directly:
- `Row`, `Column`, `Flex`, `Stack`, `Wrap`
- `ListView`, `GridView`, `SingleChildScrollView`
- `Text`, `RichText`, `Image.network`, `Image.asset`
- `Icon` (with `A2UIIcons` mapping)
- `Container`, `SizedBox`, `Padding`, `Center`, `Align`
- `GestureDetector`, `MouseRegion`, `AnimatedContainer`
- `ClipRRect`, `BackdropFilter`, `Opacity`

### 2. New Component Checklist

When adding a new `Osc*` widget to the package, you **MUST** complete ALL of these:

- [ ] **Create the widget** in `packages/oscortex_ui/lib/src/widgets/osc_<name>.dart`
- [ ] **Add `@OscPreview` annotations** with at least 2 preview functions
- [ ] **Export in barrel** — add to `lib/oscortex_ui.dart`
- [ ] **Map in A2UI renderer** — add a case in `a2ui_renderer.dart` `_renderComponent()`
- [ ] **Add A2UI component type** — add enum value in `a2ui_types.dart` if new
- [ ] **Add parser support** — handle in `a2ui_parser.dart` `_parseType()` and `_parseProps()`
- [ ] **Run `flutter analyze`** on `packages/oscortex_ui` — must be 0 issues

### 3. A2UI Renderer Architecture

```
packages/oscortex_ui/lib/src/
├── a2ui/
│   ├── a2ui.dart              ← barrel export
│   ├── a2ui_types.dart        ← Component, BoundValue, Action, Surface
│   ├── a2ui_parser.dart       ← JSON → A2UISurface (v0.8 + v0.9)
│   ├── a2ui_data_store.dart   ← Reactive path-based data binding
│   ├── a2ui_icons.dart        ← Icon name → Material IconData
│   ├── a2ui_renderer.dart     ← Component → Osc* widget mapping
│   └── a2ui_surface.dart      ← Top-level JSON → UI widget
└── widgets/
    ├── osc_button.dart        ← A2UI Button
    ├── osc_checkbox.dart      ← A2UI CheckBox
    ├── osc_slider.dart        ← A2UI Slider
    ├── osc_date_picker.dart   ← A2UI DateTimeInput
    ├── osc_modal.dart         ← A2UI Modal
    ├── osc_tabs.dart          ← A2UI Tabs
    ├── osc_text_field.dart    ← A2UI TextField
    ├── osc_card.dart          ← A2UI Card
    ├── osc_chip.dart          ← A2UI MultipleChoice/ChoicePicker
    └── ...                    ← All other Osc* widgets
```

### 4. Current A2UI Component Support

| A2UI Component     | Osc* Widget      | Status |
|--------------------|------------------|--------|
| Row                | Row (passthrough) | ✅ |
| Column             | Column (passthrough) | ✅ |
| List               | ListView (passthrough) | ✅ |
| Text               | Text + OscTypography | ✅ |
| Image              | Image.network (passthrough) | ✅ |
| Icon               | A2UIIcons → Icon | ✅ |
| Divider            | Container (passthrough) | ✅ |
| Button             | OscButton / OscOutlineButton | ✅ |
| TextField          | OscTextField | ✅ |
| CheckBox           | OscCheckbox | ✅ |
| Slider             | OscSlider | ✅ |
| DateTimeInput      | OscDatePicker | ✅ |
| MultipleChoice     | OscChip | ✅ |
| Card               | OscCard | ✅ |
| Modal              | OscModal | ✅ |
| Tabs               | OscTabs | ✅ |

### 5. Testing

After any changes:

```bash
cd packages/oscortex_ui && flutter analyze
cd apps/oscortex_app && flutter analyze
cd apps/oscortex_files && flutter analyze
cd apps/oscortex_canvas && flutter analyze
cd apps/oscortex_web_link && flutter analyze
```

All must report **0 issues**.
