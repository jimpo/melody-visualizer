# Development

Quick-start context for AI agents and developers working on Melody Visualizer.

For the design of the system — components, threads, channels — see
[ARCHITECTURE.md](ARCHITECTURE.md).

## System dependencies

The crate links against GTK 4 and JACK. On Debian/Ubuntu:

```bash
sudo apt install build-essential clang libgtk-4-dev libjack-jackd2-dev
```

To run the app headlessly (see [below](#running-the-gui-headlessly)) you also
need:

```bash
sudo apt install jackd2 xvfb dbus-x11 imagemagick
```

`claude-sbx.Dockerfile` provisions all of the above for the agent sandbox.

## Build Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run                      # Needs a display and a running JACK server
```

## Testing

Unit tests live in `#[cfg(test)]` modules under `src/`; integration tests live in
`tests/` and reach the crate as a library (`use melody_visualizer::…`). The
`test_support` fixtures are public to `tests/` and `benches/` through the
`testing` feature, which the self dev-dependency in `Cargo.toml` enables for
those targets — no extra flag is needed on the command line.

Run the tests with [cargo-nextest](https://nexte.st/). It executes each test in
its own process, which this crate **requires** — see the warning below.

```bash
$ cargo install cargo-nextest --locked   # one-time setup

$ cargo nextest run                      # Run the test suite
$ cargo nextest run -E 'test(pubsub)'    # Run the tests matching a filter expression
$ cargo nextest run --run-ignored all    # Include the tests that need a JACK server
```

> **`cargo test` currently aborts, even though every test passes.** Several tests
> (`pubsub::tests::*`, `audio::tests::notification_handler_*`) drive a glib main
> loop through `test_support::run_in_glib_main_loop`, and that loop runs on the
> process-global default `MainContext`. libtest runs many tests in one process,
> so the second such test either touches a value glib pinned to another thread or
> polls a task the first test left behind. Either way glib panics inside a
> non-unwinding context and the process takes a `SIGABRT`. nextest's
> process-per-test isolation sidesteps it entirely.
>
> The real fix is for `run_in_glib_main_loop` to build a fresh `MainContext`
> instead of using the thread default, and to drop its leftover yield task when
> the test ends. Until then, use nextest.

Tests that need a live JACK server are marked `#[ignore]`. Start a dummy server
first:

```bash
$ jackd -r -d dummy &
$ cargo nextest run --run-ignored all
```

## Running automated checks

```bash
$ cargo fmt                                    # rustfmt.toml pins hard_tabs = true
$ cargo fmt --check                            # verify without rewriting
$ cargo clippy --all-targets -- -D warnings
```

Formatting and clippy are both clean today. Keep them that way: run both before
you hand work back.

One lint is suppressed, at `gui::window::shutdown` — the doc comment there says
why. Suppress a lint only where fixing it would mean a refactor beyond the
change at hand, and always with the reason in a comment.

The codebase is indented with **hard tabs**. `rustfmt.toml` enforces this, so run
`cargo fmt` rather than matching it by hand.

## Running the GUI headlessly

The app is a GUI, but it runs and can be visually verified with **no physical
display and no audio hardware**. Two things must exist first, because the app
hard-requires both at startup:

1. **A display.** GTK needs an X server. A virtual framebuffer (**Xvfb**) is
   enough — the visualization is drawn with CPU cairo, so no GPU or GL is needed.
2. **A running JACK server.** The client is opened with `NO_START_SERVER`, so
   `AppController::new()` errors out before the window is built if no server is
   up. A **dummy-backend** `jackd -d dummy` touches no hardware and still gives
   the app real `system:capture_*` ports to enumerate.

`scripts/run-headless.sh` automates the whole flow. It brings up `jackd -d dummy`
and `Xvfb` (idempotently), builds, launches the app under `dbus-run-session` (so
GApplication has a session bus to register on, else it hangs), waits, screenshots
the framebuffer with ImageMagick `import`, and stops the app again.

```bash
$ scripts/run-headless.sh          # -> /tmp/melody-shot.png
```

| Env var | Meaning |
|---|---|
| `SHOT` | Screenshot output path (default `/tmp/melody-shot.png`) |
| `WAIT` | Seconds to wait before capturing (default 8) |
| `KEEP_RUNNING=1` | Leave the app up, e.g. to drive it or take several shots |
| `SCREEN` | Xvfb geometry (default `1600x1000x24`) |
| `APP_LOG` | App stdout/stderr with `RUST_LOG=debug` (default `/tmp/melody-app.log`) |

`Audio source activated` in the app log is the signal that the JACK layer
connected. A panic at `window.rs`'s `.expect("failed to create main window")`
instead means JACK was not reachable — a distinct failure from a display problem.

### What a plain run proves, and what it does not

A successful run proves GTK rendered the full UI chrome and the JACK port list —
the screenshot shows the control pane listing `system:capture_*`. On its own it
does **not** prove the visualizer animates: with no signal connected the spectrum
is silent, so the spiral renders at its static base brightness.

### Driving the live pipeline

To exercise audio → spectrum → graphic end to end, feed the app's input a real
signal with `scripts/connect-test-tone.sh`. It starts a `jack_simple_client` sine
and connects it to `Melody Visualizer:input`.

```bash
$ KEEP_RUNNING=1 scripts/run-headless.sh   # launch the app, leave it running
$ scripts/connect-test-tone.sh             # connect a sine tone
$ DISPLAY=:99 import -window root /tmp/tone.png
```

The spiral lights one bright band at the tone's frequency — a pure sine gives one
band. `scripts/connect-test-tone.sh --disconnect` stops driving the input.

## Known upstream workarounds

One workaround is load-bearing. It should be re-checked against current upstream
before being copied or extended.

| Where | Why |
|---|---|
| `controllers/control_pane.rs` — a 10 ms poll after a port-unregister event | [jack2#617](https://github.com/jackaudio/jack2/issues/617). The port list is not immediately consistent after the notification. |

## Key Terminology

| Term | Definition |
|---|---|
| **Overrun** | Audio the RT thread dropped because the capture ring was full. Counted in an atomic on `SampleWriter`, read off `SampleReader::overruns` on the spectrum thread and logged as a warning |
| **Spectrum** | One frame of frequency-domain data: non-negative power values plus the `SpectrumParams` that give each bin its frequency |
| **SpectrumParams** | The log-spaced frequency grid. Shared as an `Arc` and compared with `Arc::ptr_eq`; a pointer mismatch is what invalidates downstream caches |
| **SpectrumBuffer** | A `Spectrum` with no meaningful contents — the recycled allocation that cycles back from the graphic thread |
| **Spectrum transform** | One stage of the DSP chain (`Diffuser`, `VolumeNormalizer`, `DecibelConverter`), applied in a configured order |
| **Generator** | The head of a pipeline. A `SpectrumGenerator` makes a spectrum from samples; a `GraphicGenerator` makes a graphic from spectra |
| **Spectrum tick** | The spectrum thread's timer, at `generator.interval()` — half a DFT window. It is the pipeline's only timed producer |
| **Frame timer** | The GTK thread's 40 ms timer that requests a render. Independent of the spectrum tick |
| **`AsyncProcessor<T>`** | The RPC handle. Ships a `FnOnce(&mut T)` to the thread that owns `T` and awaits the result |
| **PubSub / `Notifier`** | The event bus. Any thread may publish; every subscriber callback runs on the GTK thread |
| **Controller / view** | Controllers own state and live in `Rc<RefCell<…>>`; views build widgets and hold none |

## Documentation & Resources

### Architecture
[ARCHITECTURE.md](ARCHITECTURE.md) is the design document: the five components,
the concurrency model, the data and control paths, the invariants that hold the
design together, and the list of known gaps between the code and the intent.

### README
[README.md](README.md) is the project's entry point.

### Working conventions
`CLAUDE.local.md` (not tracked) holds machine-local guidance: the Linear project
this repo is driven from, the branch and merge workflow, and how work is handed
back to the host machine.
