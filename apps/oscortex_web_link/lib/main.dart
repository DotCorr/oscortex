import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

void main() {
  runApp(const WebLinkApp());
}

class WebLinkApp extends StatelessWidget {
  const WebLinkApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: OscTheme.dark(accent: OscColors.green),
      home: const WebLinkHome(),
    );
  }
}

class WebLinkHome extends StatefulWidget {
  const WebLinkHome({super.key});

  @override
  State<WebLinkHome> createState() => _WebLinkHomeState();
}

class _WebLinkHomeState extends State<WebLinkHome> {
  final _controller =
      TextEditingController(text: 'https://dccortec.com/docs/arch');

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: OscColors.bodyBg,
      body: Stack(
        children: [
          // ── Subtle emerald radial gradient background
          Positioned.fill(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: RadialGradient(
                  center: const Alignment(-0.6, -0.8),
                  radius: 1.4,
                  colors: [
                    OscColors.green.withValues(alpha: 0.045),
                    OscColors.bodyBg.withValues(alpha: 0.0),
                  ],
                  stops: const [0.0, 0.7],
                ),
              ),
            ),
          ),
          // ── Second subtle gradient from bottom-right
          Positioned.fill(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: RadialGradient(
                  center: const Alignment(0.9, 1.0),
                  radius: 1.2,
                  colors: [
                    OscColors.violet.withValues(alpha: 0.025),
                    Colors.transparent,
                  ],
                  stops: const [0.0, 0.6],
                ),
              ),
            ),
          ),
          // ── Main content
          SafeArea(
            child: SingleChildScrollView(
              padding: const EdgeInsets.symmetric(horizontal: 28, vertical: 24),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // ── Title
                  Text(
                    'Web Link',
                    style: OscTypography.heading1.copyWith(
                      color: OscColors.textPrimary,
                      letterSpacing: -0.5,
                      height: 1.1,
                    ),
                  ),
                  const SizedBox(height: 6),
                  Row(
                    children: [
                      Container(
                        width: 6,
                        height: 6,
                        decoration: const BoxDecoration(
                          color: OscColors.green,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Text(
                        'Knowledge source & media gateway',
                        style: OscTypography.caption.copyWith(
                          fontSize: 14,
                          color: OscColors.textMuted,
                          fontWeight: FontWeight.w400,
                          letterSpacing: 0.2,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 28),

                  // ── URL Input
                  TextField(
                    controller: _controller,
                    style: OscTypography.monoBody.copyWith(
                      color: OscColors.textPrimary,
                      fontSize: 14,
                    ),
                    decoration: const InputDecoration(
                      labelText: 'Link target',
                      prefixIcon: Icon(Icons.link_rounded),
                    ),
                  ),
                  const SizedBox(height: 16),

                  // ── Open Link button
                  FilledButton.icon(
                    onPressed: () {
                      final url = _controller.text.trim();
                      ScaffoldMessenger.of(context).showSnackBar(
                        SnackBar(
                          content: Row(
                            children: [
                              const Icon(Icons.open_in_new_rounded,
                                  color: OscColors.green, size: 16),
                              const SizedBox(width: 10),
                              Expanded(
                                child: Text(
                                  'Queued link: $url',
                                  overflow: TextOverflow.ellipsis,
                                ),
                              ),
                            ],
                          ),
                        ),
                      );
                    },
                    icon: const Icon(Icons.open_in_new_rounded, size: 18),
                    label: const Text('Open Link'),
                  ),
                  const SizedBox(height: 32),

                  // ── Knowledge Source Engine Card
                  _buildKnowledgeSourceCard(),
                  const SizedBox(height: 20),

                  // ── Media Player Card
                  _buildMediaPlayerCard(),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildKnowledgeSourceCard() {
    return OscCard(
      padding: EdgeInsets.zero,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // ── Card header bar
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: OscColors.border, width: 1),
              ),
            ),
            child: Row(
              children: [
                OscTag.green(label: 'KNOWLEDGE SOURCE ENGINE'),
                const Spacer(),
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    color: OscColors.green,
                    shape: BoxShape.circle,
                    boxShadow: [
                      BoxShadow(
                        color: OscColors.green.withValues(alpha: 0.5),
                        blurRadius: 6,
                        spreadRadius: 1,
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                Text(
                  'LIVE',
                  style: OscTypography.monoLabel.copyWith(
                    color: OscColors.green,
                    letterSpacing: 1.0,
                  ),
                ),
              ],
            ),
          ),

          // ── Card content
          Padding(
            padding: const EdgeInsets.all(18),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // URL preview
                Row(
                  children: [
                    const Icon(Icons.language_rounded,
                        color: OscColors.skyBlue, size: 14),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        _controller.text,
                        style: OscTypography.monoBody.copyWith(
                          fontSize: 12,
                          color: OscColors.skyBlue,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 16),

                // Page title
                Text(
                  'Operating Systems Architecture Evolution',
                  style: OscTypography.heading3.copyWith(
                    fontSize: 18,
                    fontWeight: FontWeight.w700,
                    color: OscColors.textPrimary,
                    height: 1.3,
                  ),
                ),
                const SizedBox(height: 10),

                // Description
                Text(
                  'Comprehensive guide covering microkernel design principles, '
                  'memory-safe driver isolation, capability-based security models, '
                  'and the convergence of embedded real-time constraints with '
                  'modern desktop compositing pipelines. Explores how OSCortex '
                  'synthesizes these paradigms into a unified bare-metal runtime.',
                  style: OscTypography.body.copyWith(
                    color: OscColors.textMuted,
                    height: 1.65,
                    fontWeight: FontWeight.w400,
                  ),
                ),
                const SizedBox(height: 18),

                // Meta tags row
                Row(
                  children: [
                    _metaTag(Icons.schedule_rounded, '12 min read'),
                    const SizedBox(width: 10),
                    _metaTag(Icons.bookmark_outline_rounded, 'Saved'),
                    const Spacer(),
                    OscTag.green(label: 'Architecture'),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildMediaPlayerCard() {
    return OscCard(
      padding: EdgeInsets.zero,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // ── Header bar
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: OscColors.border, width: 1),
              ),
            ),
            child: Row(
              children: [
                OscTag.violet(label: 'NOW PLAYING'),
                const Spacer(),
                Icon(Icons.volume_up_rounded,
                    size: 16, color: OscColors.violetLight.withValues(alpha: 0.6)),
              ],
            ),
          ),

          // ── Body
          Padding(
            padding: const EdgeInsets.all(18),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // Track info
                Text(
                  'Ambient Waves',
                  style: OscTypography.heading3.copyWith(
                    fontSize: 17,
                    fontWeight: FontWeight.w700,
                    color: OscColors.textPrimary,
                    height: 1.2,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  'Cortex Generative Radio',
                  style: OscTypography.body.copyWith(
                    color: OscColors.textMuted,
                    fontWeight: FontWeight.w400,
                  ),
                ),
                const SizedBox(height: 20),

                // Waveform visualiser
                const Center(
                  child: OscWaveform(
                    barCount: 8,
                    height: 48,
                    barWidth: 6,
                    barGap: 3,
                  ),
                ),
                const SizedBox(height: 20),

                // Tags
                Row(
                  children: [
                    OscTag.violet(
                      label: 'Local Sync',
                      icon: Icons.sync_rounded,
                    ),
                    const SizedBox(width: 10),
                    OscTag.violet(
                      label: 'Spatial Audio Active',
                      icon: Icons.spatial_audio_off_rounded,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _metaTag(IconData icon, String label) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 13, color: OscColors.textMuted),
        const SizedBox(width: 5),
        Text(
          label,
          style: OscTypography.caption.copyWith(
            fontWeight: FontWeight.w500,
          ),
        ),
      ],
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Web Link App', group: 'Apps')
Widget webLinkAppPreview() {
  return const WebLinkApp();
}

@OscPreview(name: 'Web Link Home', group: 'Apps')
Widget webLinkHomePreview() {
  return const WebLinkHome();
}
