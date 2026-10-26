# Melody Visualizer

A real-time music visualizer for Linux that draws the pitches it hears on a
spiral, one turn per octave.

The app is being rebuilt as a CLAP plugin on
[nice-plug](https://codeberg.org/RustAudio/nice-plug), with a standalone JACK
build from the same code. The GTK 4 app it replaces lives in
[legacy/](legacy/README.md) as a reference until the port is done; it still
builds and runs on its own from that directory.

## Building

You need a stable Rust toolchain that supports edition 2024, and the ALSA
development headers for the standalone build's ALSA backend. The standalone
loads JACK at run time, so it needs no JACK headers to build:

```bash
sudo apt install build-essential pkg-config libasound2-dev
```

Bundle the CLAP plugin and install it where hosts look for plugins:

```bash
cargo xtask bundle melody-visualizer --release
mkdir -p ~/.clap
ln -sf "$PWD/target/bundled/Melody Visualizer.clap" ~/.clap/
```

Each `cargo xtask bundle` overwrites the bundle in place, so the link keeps
pointing at the latest build; the host only has to rescan or reload it. Set `NICE_LOG=/path/to/file` in the host's environment
to capture the plugin's log.

The plugin's window is drawn with [iced](https://iced.rs/) through
[baseview](https://github.com/RustAudio/baseview), which supports only X11 on
Linux, so the host has to run under X11 or XWayland.

Run the standalone app under a JACK server, connecting its input with
`--connect-jack-inputs` or any patchbay:

```bash
cargo run --release -- --help
```

To see the window without a display, `scripts/screenshot.sh [out.png]` runs
the standalone app under Xvfb on a private dummy JACK server with a sine tone
playing into it, and saves a screenshot of its window. It needs `jackd2`,
`xvfb` and `imagemagick`.
