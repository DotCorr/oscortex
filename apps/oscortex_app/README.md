Userspace shell surface (Flutter + oscortex-host embedder)

This app is the OSCortex shell process, not a container for all other apps.

- Core pinned apps: Canvas, Files, Web Link.
- Launch model: every launched app runs in its own process (dedicated oscortex-host PID).
- App source: kernel app registry (`app_list`, `app_launch`) exposed over the shell channel.

Runtime app install flow:

1. Build app bundle as `.osx` via `tools/build-flutter-osx.sh`.
2. Use Files core app to locate bundle and install.
3. Refresh shell list and launch from shell grid/dock.

