# Melody Visualizer

A real-time music visualizer for Linux that draws the pitches it hears on a
spiral, one turn per octave.

The app is being rebuilt as a CLAP plugin on
[nice-plug](https://codeberg.org/RustAudio/nice-plug), with a standalone JACK
build from the same code. The GTK 4 app it replaces lives in
[legacy/](legacy/README.md) as a reference until the port is done; it still
builds and runs on its own from that directory.
