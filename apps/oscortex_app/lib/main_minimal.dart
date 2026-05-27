// Minimal test app — no animation, no widget_previews import.
// Used to verify Flutter can render any pixels at all in OSCortex.
import 'package:flutter/material.dart';

void main() {
  runApp(
    MaterialApp(
      debugShowCheckedModeBanner: false,
      home: Scaffold(
        backgroundColor: const Color(0xFF0C1C26),
        body: Center(
          child: Container(
            width: 400,
            height: 200,
            color: const Color(0xFFFF4444),
            child: const Center(
              child: Text(
                'OSCortex',
                style: TextStyle(
                  color: Colors.white,
                  fontSize: 48,
                  fontWeight: FontWeight.bold,
                ),
              ),
            ),
          ),
        ),
      ),
    ),
  );
}
