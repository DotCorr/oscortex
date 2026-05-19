import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

void main() {
  runApp(const OSCortexApp());
}

class OSCortexApp extends StatelessWidget {
  const OSCortexApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OSCortex',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark().copyWith(
        scaffoldBackgroundColor: const Color(0xFF0D1117),
        colorScheme: const ColorScheme.dark(
          primary: Color(0xFF3B82F6),
          surface: Color(0xFF161B22),
        ),
      ),
      home: const LauncherScreen(),
    );
  }
}

class AppEntry {
  final String name;
  final IconData icon;
  final Color color;
  const AppEntry(this.name, this.icon, this.color);
}

const _apps = [
  AppEntry('Terminal', Icons.terminal,     Color(0xFF3FB950)),
  AppEntry('Files',    Icons.folder_open,  Color(0xFF79C0FF)),
  AppEntry('Settings', Icons.settings,     Color(0xFF8B949E)),
  AppEntry('Editor',   Icons.code,         Color(0xFFF78166)),
  AppEntry('Flutter',  Icons.flutter_dash, Color(0xFF3B82F6)),
  AppEntry('About',    Icons.info_outline, Color(0xFFC084FC)),
];

class LauncherScreen extends StatefulWidget {
  const LauncherScreen({super.key});
  @override
  State<LauncherScreen> createState() => _LauncherScreenState();
}

class _LauncherScreenState extends State<LauncherScreen> {
  int _hovered = -1;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Column(
        children: [
          _buildTaskbar(),
          Expanded(child: _buildGrid()),
          _buildDock(),
        ],
      ),
    );
  }

  Widget _buildTaskbar() {
    return Container(
      height: 40,
      color: const Color(0xFF161B22),
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: Row(
        children: [
          Container(
            width: 24, height: 24,
            decoration: BoxDecoration(
              color: const Color(0xFF3B82F6),
              borderRadius: BorderRadius.circular(4),
            ),
            child: const Icon(Icons.memory, size: 14, color: Colors.white),
          ),
          const SizedBox(width: 10),
          const Text('OSCortex',
            style: TextStyle(color: Color(0xFFE6EDF3),
              fontWeight: FontWeight.w600, fontSize: 13)),
          const SizedBox(width: 24),
          const Text('Phase 62',
            style: TextStyle(color: Color(0xFF8B949E), fontSize: 12)),
          const Spacer(),
          const Text('Desktop',
            style: TextStyle(color: Color(0xFF8B949E), fontSize: 12)),
        ],
      ),
    );
  }

  Widget _buildGrid() {
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        const Text('Welcome to OSCortex',
          style: TextStyle(color: Color(0xFFE6EDF3), fontSize: 18,
            fontWeight: FontWeight.w600)),
        const SizedBox(height: 4),
        const Text('AI-First Operating System  ·  Phase 62',
          style: TextStyle(color: Color(0xFF8B949E), fontSize: 12)),
        const SizedBox(height: 40),
        Wrap(
          spacing: 16,
          runSpacing: 16,
          alignment: WrapAlignment.center,
          children: List.generate(_apps.length, (i) {
            final app = _apps[i];
            final isHov = _hovered == i;
            return MouseRegion(
              onEnter: (_) => setState(() => _hovered = i),
              onExit:  (_) => setState(() => _hovered = -1),
              child: GestureDetector(
                onTap: () => _launch(i),
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 150),
                  width: 120, height: 120,
                  decoration: BoxDecoration(
                    color: isHov
                        ? const Color(0xFF2D3748)
                        : const Color(0xFF21262D),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                      color: isHov ? app.color : const Color(0xFF2D333B),
                      width: isHov ? 1.5 : 1.0,
                    ),
                  ),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(app.icon, size: 36, color: app.color),
                      const SizedBox(height: 10),
                      Text(app.name,
                        style: const TextStyle(
                          color: Color(0xFFE6EDF3),
                          fontSize: 12,
                          fontWeight: FontWeight.w500,
                        )),
                    ],
                  ),
                ),
              ),
            );
          }),
        ),
      ],
    );
  }

  Widget _buildDock() {
    return Container(
      height: 56,
      color: const Color(0xFF161B22),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          for (int i = 0; i < _apps.length; i++)
            Tooltip(
              message: _apps[i].name,
              child: InkWell(
                onTap: () => _launch(i),
                borderRadius: BorderRadius.circular(8),
                child: Container(
                  width: 40, height: 40,
                  margin: const EdgeInsets.symmetric(horizontal: 4),
                  decoration: BoxDecoration(
                    color: const Color(0xFF21262D),
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: Icon(_apps[i].icon, size: 20, color: _apps[i].color),
                ),
              ),
            ),
          const SizedBox(width: 24),
          const Text('Click an app to launch',
            style: TextStyle(color: Color(0xFF8B949E), fontSize: 11)),
        ],
      ),
    );
  }

  void _launch(int index) {
    final app = _apps[index];
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('Launching ${app.name}…',
          style: const TextStyle(color: Color(0xFFE6EDF3))),
        backgroundColor: const Color(0xFF21262D),
        duration: const Duration(seconds: 1),
      ),
    );
    const MethodChannel('oscortex/launch')
        .invokeMethod('launch', {'app': app.name.toLowerCase()})
        .catchError((_) {});
  }
}
