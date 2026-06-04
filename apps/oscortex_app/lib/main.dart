import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

import 'widgets/shell_desktop.dart';

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
      theme: OscTheme.dark(),
      home: const ShellDesktop(accentColor: OscColors.violet),
    );
  }
}

