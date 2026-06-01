import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/spacing.dart';
import '../tokens/typography.dart';
import '../widgets/osc_button.dart';
import '../widgets/osc_card.dart';
import '../widgets/osc_chip.dart';
import '../widgets/osc_text_field.dart';
import 'a2ui_data_store.dart';
import 'a2ui_icons.dart';
import 'a2ui_types.dart';

/// Callback when an A2UI action is triggered by a Button.
typedef A2UIActionCallback = void Function(A2UIAction action);

/// Renders an [A2UIComponent] tree into Flutter widgets using Osc* components.
///
/// The renderer walks the component tree, resolves data bindings from the
/// [A2UIDataStore], and maps each A2UI component type to the appropriate
/// OSCortex design system widget.
///
/// ```dart
/// A2UIRenderer(
///   surface: parsedSurface,
///   store: dataStore,
///   onAction: (action) => agent.dispatch(action.name, action.params),
/// );
/// ```
class A2UIRenderer extends StatefulWidget {
  const A2UIRenderer({
    super.key,
    required this.surface,
    required this.store,
    this.onAction,
  });

  final A2UISurface surface;
  final A2UIDataStore store;
  final A2UIActionCallback? onAction;

  @override
  State<A2UIRenderer> createState() => _A2UIRendererState();
}

class _A2UIRendererState extends State<A2UIRenderer> {
  @override
  void initState() {
    super.initState();
    widget.store.addListener(_onDataChanged);
  }

  @override
  void didUpdateWidget(A2UIRenderer old) {
    super.didUpdateWidget(old);
    if (old.store != widget.store) {
      old.store.removeListener(_onDataChanged);
      widget.store.addListener(_onDataChanged);
    }
  }

  @override
  void dispose() {
    widget.store.removeListener(_onDataChanged);
    super.dispose();
  }

  void _onDataChanged() => setState(() {});

  @override
  Widget build(BuildContext context) {
    return _renderComponent(widget.surface.rootId);
  }

  // ── Core renderer ─────────────────────────────────────────────────

  Widget _renderComponent(String id) {
    final comp = widget.surface[id];
    if (comp == null) {
      return const SizedBox.shrink();
    }

    Widget child = switch (comp.type) {
      A2UIComponentType.row => _buildRow(comp),
      A2UIComponentType.column => _buildColumn(comp),
      A2UIComponentType.list => _buildList(comp),
      A2UIComponentType.text => _buildText(comp),
      A2UIComponentType.image => _buildImage(comp),
      A2UIComponentType.icon => _buildIcon(comp),
      A2UIComponentType.divider => _buildDivider(comp),
      A2UIComponentType.button => _buildButton(comp),
      A2UIComponentType.textField => _buildTextField(comp),
      A2UIComponentType.checkBox => _buildCheckBox(comp),
      A2UIComponentType.slider => _buildSlider(comp),
      A2UIComponentType.dateTimeInput => _buildDateTimeInput(comp),
      A2UIComponentType.multipleChoice => _buildMultipleChoice(comp),
      A2UIComponentType.card => _buildCard(comp),
      A2UIComponentType.modal => _buildModal(comp),
      A2UIComponentType.tabs => _buildTabs(comp),
    };

    // Apply flex weight if inside a Row/Column
    if (comp.weight != null && comp.weight! > 0) {
      child = Expanded(flex: comp.weight!.round(), child: child);
    }

    // Apply accessibility label
    if (comp.accessibility?.label != null) {
      child = Semantics(
        label: comp.accessibility!.label,
        child: child,
      );
    }

    return child;
  }

  List<Widget> _resolveChildren(A2UIChildren? children) {
    if (children == null) return [];
    if (children.isExplicit) {
      return children.ids!.map(_renderComponent).toList();
    }
    if (children.isTemplate && children.templateBinding != null) {
      final list = widget.store.get(children.templateBinding!) as List?;
      if (list == null) return [];
      return List.generate(list.length, (i) {
        return _renderComponent(children.templateComponentId ?? '');
      });
    }
    return [];
  }

  // ── Layout Components ─────────────────────────────────────────────

  Widget _buildRow(A2UIComponent comp) {
    final kids = _resolveChildren(comp.children);
    return Row(
      mainAxisAlignment: _mainAxis(comp.distribution),
      crossAxisAlignment: _crossAxis(comp.alignment),
      mainAxisSize: MainAxisSize.min,
      children: kids,
    );
  }

  Widget _buildColumn(A2UIComponent comp) {
    final kids = _resolveChildren(comp.children);
    return Column(
      mainAxisAlignment: _mainAxis(comp.distribution),
      crossAxisAlignment: _crossAxis(comp.alignment),
      mainAxisSize: MainAxisSize.min,
      children: kids,
    );
  }

  Widget _buildList(A2UIComponent comp) {
    final kids = _resolveChildren(comp.children);
    final isHorizontal = comp.direction == 'horizontal';
    return ListView(
      scrollDirection: isHorizontal ? Axis.horizontal : Axis.vertical,
      shrinkWrap: true,
      children: kids,
    );
  }

  // ── Display Components ────────────────────────────────────────────

  Widget _buildText(A2UIComponent comp) {
    final resolved = comp.text?.resolve(widget.store.data) ?? '';
    final style = _textStyle(comp.usageHint);
    return Text(resolved, style: style);
  }

  Widget _buildImage(A2UIComponent comp) {
    final imageUrl = comp.url?.resolve(widget.store.data) ?? '';
    if (imageUrl.isEmpty) return const SizedBox.shrink();

    final boxFit = switch (comp.fit) {
      'cover' => BoxFit.cover,
      'contain' => BoxFit.contain,
      'fill' => BoxFit.fill,
      _ => BoxFit.cover,
    };

    return ClipRRect(
      borderRadius: OscRadii.innerBorder,
      child: Image.network(
        imageUrl,
        fit: boxFit,
        errorBuilder: (_, __, ___) => Container(
          height: 120,
          color: OscColors.surface,
          child: const Center(
            child: Icon(Icons.broken_image_rounded,
                size: 32, color: OscColors.textDim),
          ),
        ),
      ),
    );
  }

  Widget _buildIcon(A2UIComponent comp) {
    final name = comp.iconName?.resolve(widget.store.data) ?? 'circle';
    return Icon(
      A2UIIcons.resolve(name),
      size: 20,
      color: OscColors.textMuted,
    );
  }

  Widget _buildDivider(A2UIComponent comp) {
    if (comp.axis == 'vertical') {
      return Container(
        width: 1,
        margin: const EdgeInsets.symmetric(horizontal: OscSpacing.md),
        color: OscColors.border,
      );
    }
    return Container(
      height: 1,
      margin: const EdgeInsets.symmetric(vertical: OscSpacing.md),
      color: OscColors.border,
    );
  }

  // ── Interactive Components ────────────────────────────────────────

  Widget _buildButton(A2UIComponent comp) {
    // Resolve child content (typically an A2UI Text component)
    Widget childWidget = const SizedBox.shrink();
    if (comp.childId != null) {
      childWidget = _renderComponent(comp.childId!);
    }

    final isPrimary = comp.primary || comp.variant == 'primary';
    final onPressed = comp.action != null
        ? () => widget.onAction?.call(comp.action!)
        : null;

    if (isPrimary) {
      return FilledButton(
        onPressed: onPressed,
        style: FilledButton.styleFrom(
          backgroundColor: OscColors.violet,
          foregroundColor: Colors.white,
          shape: RoundedRectangleBorder(
              borderRadius: OscRadii.buttonBorder),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        ),
        child: childWidget,
      );
    }

    return OutlinedButton(
      onPressed: onPressed,
      style: OutlinedButton.styleFrom(
        foregroundColor: OscColors.textPrimary,
        side: BorderSide(color: Colors.white.withValues(alpha: 0.15)),
        shape: RoundedRectangleBorder(
            borderRadius: OscRadii.buttonBorder),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      ),
      child: childWidget,
    );
  }

  Widget _buildTextField(A2UIComponent comp) {
    final labelText = comp.label?.resolve(widget.store.data);
    final currentValue = comp.value?.resolve(widget.store.data) ?? '';

    return OscTextField(
      hint: labelText,
      accent: OscColors.violet,
      controller: TextEditingController(text: currentValue),
      onChanged: comp.value?.isBinding == true
          ? (val) => widget.store.set(comp.value!.path!, val)
          : null,
    );
  }

  Widget _buildCheckBox(A2UIComponent comp) {
    final labelText = comp.label?.resolve(widget.store.data) ?? '';
    final checked =
        comp.value?.resolveTyped<bool>(widget.store.data) ?? false;

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: 20,
          height: 20,
          child: Checkbox(
            value: checked,
            activeColor: OscColors.violet,
            checkColor: Colors.white,
            side: const BorderSide(color: OscColors.border),
            shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(4)),
            onChanged: comp.value?.isBinding == true
                ? (val) => widget.store.set(comp.value!.path!, val ?? false)
                : null,
          ),
        ),
        const SizedBox(width: OscSpacing.md),
        Text(labelText,
            style: OscTypography.body.copyWith(color: OscColors.textPrimary)),
      ],
    );
  }

  Widget _buildSlider(A2UIComponent comp) {
    final val =
        comp.value?.resolveTyped<double>(widget.store.data) ?? comp.minValue ?? 0;
    final min = comp.minValue ?? 0;
    final max = comp.maxValue ?? 100;

    return SliderTheme(
      data: SliderThemeData(
        activeTrackColor: OscColors.violet,
        inactiveTrackColor: Colors.white.withValues(alpha: 0.08),
        thumbColor: OscColors.violet,
        overlayColor: OscColors.violet.withValues(alpha: 0.12),
        trackHeight: 4,
      ),
      child: Slider(
        value: val.clamp(min, max),
        min: min,
        max: max,
        onChanged: comp.value?.isBinding == true
            ? (v) => widget.store.set(comp.value!.path!, v)
            : null,
      ),
    );
  }

  Widget _buildDateTimeInput(A2UIComponent comp) {
    final currentVal = comp.value?.resolve(widget.store.data) ?? '';
    return OscButton(
      label: currentVal.isNotEmpty ? currentVal : 'Select date…',
      icon: Icons.calendar_today_rounded,
      accent: OscColors.skyBlue,
      onPressed: () async {
        if (!comp.enableDate) return;
        final date = await showDatePicker(
          context: context,
          initialDate: DateTime.now(),
          firstDate: DateTime(2000),
          lastDate: DateTime(2100),
          builder: (ctx, child) => Theme(
            data: Theme.of(ctx).copyWith(
              colorScheme: ColorScheme.dark(
                primary: OscColors.violet,
                surface: OscColors.canvasBg,
              ),
            ),
            child: child ?? const SizedBox.shrink(),
          ),
        );
        if (date != null && comp.value?.isBinding == true) {
          widget.store.set(comp.value!.path!, date.toIso8601String());
        }
      },
    );
  }

  Widget _buildMultipleChoice(A2UIComponent comp) {
    final opts = comp.options ?? [];
    final maxSel = comp.maxAllowedSelections ?? opts.length;
    final selections =
        comp.selections?.resolveTyped<List>(widget.store.data) ?? [];
    final selectedSet = Set<String>.from(selections.map((e) => e.toString()));

    return Wrap(
      spacing: OscSpacing.sm,
      runSpacing: OscSpacing.sm,
      children: opts.map((opt) {
        final isSelected = selectedSet.contains(opt.value);
        return OscChip(
          label: opt.label.resolve(widget.store.data),
          selected: isSelected,
          onTap: comp.selections?.isBinding == true
              ? () {
                  final path = comp.selections!.path!;
                  final current = List<String>.from(
                      (widget.store.get(path) as List?)?.cast<String>() ?? []);
                  if (isSelected) {
                    current.remove(opt.value);
                  } else if (current.length < maxSel) {
                    current.add(opt.value);
                  } else if (maxSel == 1) {
                    current
                      ..clear()
                      ..add(opt.value);
                  }
                  widget.store.set(path, current);
                }
              : null,
        );
      }).toList(),
    );
  }

  // ── Container Components ──────────────────────────────────────────

  Widget _buildCard(A2UIComponent comp) {
    return OscCard(
      child: comp.childId != null
          ? _renderComponent(comp.childId!)
          : const SizedBox.shrink(),
    );
  }

  Widget _buildModal(A2UIComponent comp) {
    return GestureDetector(
      onTap: () {
        if (comp.contentChild == null) return;
        showDialog(
          context: context,
          builder: (_) => Dialog(
            backgroundColor: OscColors.canvasBg,
            shape: RoundedRectangleBorder(borderRadius: OscRadii.cardBorder),
            child: Padding(
              padding: const EdgeInsets.all(OscSpacing.xl5),
              child: _renderComponent(comp.contentChild!),
            ),
          ),
        );
      },
      child: comp.entryPointChild != null
          ? _renderComponent(comp.entryPointChild!)
          : const SizedBox.shrink(),
    );
  }

  Widget _buildTabs(A2UIComponent comp) {
    final items = comp.tabItems ?? [];
    if (items.isEmpty) return const SizedBox.shrink();

    return DefaultTabController(
      length: items.length,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TabBar(
            isScrollable: items.length > 3,
            indicatorColor: OscColors.violet,
            labelColor: OscColors.textPrimary,
            unselectedLabelColor: OscColors.textMuted,
            dividerColor: OscColors.border,
            tabs: items
                .map((t) => Tab(text: t.title.resolve(widget.store.data)))
                .toList(),
          ),
          Expanded(
            child: TabBarView(
              children: items.map((t) => _renderComponent(t.child)).toList(),
            ),
          ),
        ],
      ),
    );
  }

  // ── Helpers ───────────────────────────────────────────────────────

  MainAxisAlignment _mainAxis(String? dist) {
    return switch (dist) {
      'start' => MainAxisAlignment.start,
      'end' => MainAxisAlignment.end,
      'center' => MainAxisAlignment.center,
      'spaceBetween' => MainAxisAlignment.spaceBetween,
      'spaceAround' => MainAxisAlignment.spaceAround,
      'spaceEvenly' => MainAxisAlignment.spaceEvenly,
      _ => MainAxisAlignment.start,
    };
  }

  CrossAxisAlignment _crossAxis(String? align) {
    return switch (align) {
      'start' => CrossAxisAlignment.start,
      'end' => CrossAxisAlignment.end,
      'center' => CrossAxisAlignment.center,
      'stretch' => CrossAxisAlignment.stretch,
      _ => CrossAxisAlignment.start,
    };
  }

  TextStyle _textStyle(String? hint) {
    return switch (hint) {
      'h1' => OscTypography.heading1.copyWith(color: OscColors.textPrimary),
      'h2' => OscTypography.heading2.copyWith(color: OscColors.textPrimary),
      'h3' => OscTypography.heading3.copyWith(color: OscColors.textPrimary),
      'h4' => const TextStyle(
          fontSize: 16,
          fontWeight: FontWeight.w600,
          color: OscColors.textPrimary),
      'h5' => const TextStyle(
          fontSize: 14,
          fontWeight: FontWeight.w600,
          color: OscColors.textPrimary),
      'caption' => OscTypography.monoSmall.copyWith(color: OscColors.textMuted),
      _ => OscTypography.body.copyWith(color: OscColors.textPrimary),
    };
  }
}
