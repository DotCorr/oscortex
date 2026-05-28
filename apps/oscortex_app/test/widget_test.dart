import 'package:flutter_test/flutter_test.dart';
import 'package:oscortex_app/main.dart';

void main() {
  testWidgets('Shell app builds', (WidgetTester tester) async {
    await tester.pumpWidget(const OscortexShellApp());
    expect(find.text('OSCortex'), findsOneWidget);
  });
}
