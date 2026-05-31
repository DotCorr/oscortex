import 'package:flutter/material.dart';
import 'widgets/shell_desktop.dart';

const _bg = Color(0xFF07080B);
const _accent = Color(0xFF7C3AED);

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const OscortexShellApp());
}

class OscortexShellApp extends StatelessWidget {
  const OscortexShellApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        fontFamily: 'Inter',
        brightness: Brightness.dark,
        scaffoldBackgroundColor: _bg,
        colorScheme: const ColorScheme.dark(primary: _accent, surface: _bg),
        useMaterial3: true,
      ),
      home: const ShellDesktop(accentColor: _accent),
    );
  }
}

