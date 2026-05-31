import 'package:flutter/material.dart';

// ─── Design Tokens ───────────────────────────────────────────────────────────
const Color _bodyBg = Color(0xFF07080B);
const Color _canvasBg = Color(0xFF0B0D12);
const Color _surfaceCard = Color(0x06FFFFFF);
const Color _emerald = Color(0xFF10B981);
const Color _emeraldMuted = Color(0x3310B981);
const Color _violet = Color(0xFF7C3AED);
const Color _violetLight = Color(0xFFA78BFA);
const Color _skyBlue = Color(0xFF38BDF8);
const Color _border = Color(0x14FFFFFF);
const Color _textMain = Color(0xFFD4D4D8);
const Color _textMuted = Color(0xFF71717A);

void main() {
  runApp(const WebLinkApp());
}

class WebLinkApp extends StatelessWidget {
  const WebLinkApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        fontFamily: 'NotoSans',
        scaffoldBackgroundColor: _bodyBg,
        colorScheme: const ColorScheme.dark(
          primary: _emerald,
          onPrimary: Colors.white,
          surface: _canvasBg,
          onSurface: _textMain,
        ),
        useMaterial3: true,
        filledButtonTheme: FilledButtonThemeData(
          style: FilledButton.styleFrom(
            backgroundColor: _emerald,
            foregroundColor: Colors.white,
            textStyle: const TextStyle(
              fontWeight: FontWeight.w600,
              fontSize: 14,
              letterSpacing: 0.3,
            ),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(10),
            ),
            padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 14),
          ),
        ),
        inputDecorationTheme: InputDecorationTheme(
          filled: true,
          fillColor: _surfaceCard,
          contentPadding:
              const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(10),
            borderSide: const BorderSide(color: _border, width: 1),
          ),
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(10),
            borderSide: const BorderSide(color: _border, width: 1),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(10),
            borderSide: const BorderSide(color: _emerald, width: 1.5),
          ),
          labelStyle: const TextStyle(color: _textMuted, fontSize: 14),
          prefixIconColor: _emerald,
          hintStyle: const TextStyle(color: _textMuted),
        ),
        snackBarTheme: SnackBarThemeData(
          backgroundColor: _canvasBg,
          contentTextStyle: const TextStyle(color: _textMain, fontSize: 13),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(10),
            side: const BorderSide(color: _border),
          ),
          behavior: SnackBarBehavior.floating,
        ),
      ),
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
      TextEditingController(text: 'https://oscortex.local/docs/arch');

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _bodyBg,
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
                    _emerald.withValues(alpha: 0.045),
                    _bodyBg.withValues(alpha: 0.0),
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
                    _violet.withValues(alpha: 0.025),
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
                  const Text(
                    'Web Link',
                    style: TextStyle(
                      fontSize: 32,
                      fontWeight: FontWeight.w800,
                      color: _textMain,
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
                          color: _emerald,
                          shape: BoxShape.circle,
                        ),
                      ),
                      const SizedBox(width: 8),
                      const Text(
                        'Knowledge source & media gateway',
                        style: TextStyle(
                          fontSize: 14,
                          color: _textMuted,
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
                    style: const TextStyle(
                      color: _textMain,
                      fontSize: 14,
                      fontFamily: 'RobotoMono',
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
                                  color: _emerald, size: 16),
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
                  const _MediaPlayerCard(),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildKnowledgeSourceCard() {
    return Container(
      width: double.infinity,
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _border, width: 1),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // ── Card header bar
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: _border, width: 1),
              ),
            ),
            child: Row(
              children: [
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                  decoration: BoxDecoration(
                    color: _emeraldMuted,
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: const Text(
                    'KNOWLEDGE SOURCE ENGINE',
                    style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w700,
                      color: _emerald,
                      letterSpacing: 1.4,
                      fontFamily: 'RobotoMono',
                    ),
                  ),
                ),
                const Spacer(),
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    color: _emerald,
                    shape: BoxShape.circle,
                    boxShadow: [
                      BoxShadow(
                        color: _emerald.withValues(alpha: 0.5),
                        blurRadius: 6,
                        spreadRadius: 1,
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                const Text(
                  'LIVE',
                  style: TextStyle(
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                    color: _emerald,
                    letterSpacing: 1.0,
                    fontFamily: 'RobotoMono',
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
                        color: _skyBlue, size: 14),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        _controller.text,
                        style: const TextStyle(
                          fontSize: 12,
                          color: _skyBlue,
                          fontFamily: 'RobotoMono',
                          fontWeight: FontWeight.w400,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 16),

                // Page title
                const Text(
                  'Operating Systems Architecture Evolution',
                  style: TextStyle(
                    fontSize: 18,
                    fontWeight: FontWeight.w700,
                    color: _textMain,
                    height: 1.3,
                  ),
                ),
                const SizedBox(height: 10),

                // Description
                const Text(
                  'Comprehensive guide covering microkernel design principles, '
                  'memory-safe driver isolation, capability-based security models, '
                  'and the convergence of embedded real-time constraints with '
                  'modern desktop compositing pipelines. Explores how OSCortex '
                  'synthesizes these paradigms into a unified bare-metal runtime.',
                  style: TextStyle(
                    fontSize: 13,
                    color: _textMuted,
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
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 10, vertical: 5),
                      decoration: BoxDecoration(
                        color: _emeraldMuted,
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child: const Text(
                        'Architecture',
                        style: TextStyle(
                          fontSize: 11,
                          color: _emerald,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
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
        Icon(icon, size: 13, color: _textMuted),
        const SizedBox(width: 5),
        Text(
          label,
          style: const TextStyle(
            fontSize: 11,
            color: _textMuted,
            fontWeight: FontWeight.w500,
          ),
        ),
      ],
    );
  }
}

// ─── Media Player Card ───────────────────────────────────────────────────────
class _MediaPlayerCard extends StatefulWidget {
  const _MediaPlayerCard();

  @override
  State<_MediaPlayerCard> createState() => _MediaPlayerCardState();
}

class _MediaPlayerCardState extends State<_MediaPlayerCard>
    with TickerProviderStateMixin {
  static const int _barCount = 8;
  static const Duration _animDuration = Duration(milliseconds: 1200);

  late final List<AnimationController> _controllers;
  late final List<Animation<double>> _animations;

  @override
  void initState() {
    super.initState();

    _controllers = List.generate(_barCount, (i) {
      return AnimationController(
        vsync: this,
        duration: _animDuration,
      );
    });

    _animations = List.generate(_barCount, (i) {
      final curved = CurvedAnimation(
        parent: _controllers[i],
        curve: Curves.easeInOut,
      );
      return Tween<double>(begin: 0.3, end: 1.0).animate(curved);
    });

    // Start with staggered delays
    for (int i = 0; i < _barCount; i++) {
      Future.delayed(Duration(milliseconds: i * 150), () {
        if (mounted) {
          _controllers[i].repeat(reverse: true);
        }
      });
    }
  }

  @override
  void dispose() {
    for (final c in _controllers) {
      c.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _border, width: 1),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // ── Header bar
          Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: _border, width: 1),
              ),
            ),
            child: Row(
              children: [
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                  decoration: BoxDecoration(
                    color: _violet.withValues(alpha: 0.15),
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: const Text(
                    'NOW PLAYING',
                    style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w700,
                      color: _violetLight,
                      letterSpacing: 1.4,
                      fontFamily: 'RobotoMono',
                    ),
                  ),
                ),
                const Spacer(),
                Icon(Icons.volume_up_rounded,
                    size: 16, color: _violetLight.withValues(alpha: 0.6)),
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
                const Text(
                  'Ambient Waves',
                  style: TextStyle(
                    fontSize: 17,
                    fontWeight: FontWeight.w700,
                    color: _textMain,
                    height: 1.2,
                  ),
                ),
                const SizedBox(height: 4),
                const Text(
                  'Cortex Generative Radio',
                  style: TextStyle(
                    fontSize: 13,
                    color: _textMuted,
                    fontWeight: FontWeight.w400,
                  ),
                ),
                const SizedBox(height: 20),

                // Waveform visualiser
                SizedBox(
                  height: 48,
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: List.generate(_barCount, (i) {
                      return AnimatedBuilder(
                        animation: _animations[i],
                        builder: (context, child) {
                          return Container(
                            margin: const EdgeInsets.symmetric(horizontal: 3),
                            width: 6,
                            height: 48 * _animations[i].value,
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(3),
                              gradient: LinearGradient(
                                begin: Alignment.bottomCenter,
                                end: Alignment.topCenter,
                                colors: [
                                  _violet,
                                  _violetLight,
                                ],
                              ),
                              boxShadow: [
                                BoxShadow(
                                  color: _violet.withValues(
                                      alpha: 0.3 * _animations[i].value),
                                  blurRadius: 8,
                                  spreadRadius: 0,
                                ),
                              ],
                            ),
                          );
                        },
                      );
                    }),
                  ),
                ),
                const SizedBox(height: 20),

                // Tags
                Row(
                  children: [
                    _playerTag(Icons.sync_rounded, 'Local Sync'),
                    const SizedBox(width: 10),
                    _playerTag(
                        Icons.spatial_audio_off_rounded, 'Spatial Audio Active'),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _playerTag(IconData icon, String label) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: _violet.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: _violet.withValues(alpha: 0.15), width: 1),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 12, color: _violetLight),
          const SizedBox(width: 6),
          Text(
            label,
            style: const TextStyle(
              fontSize: 11,
              color: _violetLight,
              fontWeight: FontWeight.w500,
            ),
          ),
        ],
      ),
    );
  }
}

// ─── AnimatedBuilder (flutter built-in is AnimatedBuilder) ────────────────────
// Using AnimatedBuilder from flutter/material.dart which is re-exported.
