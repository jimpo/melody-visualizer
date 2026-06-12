# AGENTS.md

High-level architecture guide for the **melody-visualizer** — a real-time music
visualizer that captures audio from a [JACK](https://jackaudio.org/) server,
computes a log-frequency spectrum, and draws an animated graphic in a GTK+3
window.

This document explains the *shape* of the system: the threads, the data flow,
and the three distinct communication mechanisms that tie them together. For
build/run notes see `README.md` and `CLAUDE.md`.

---

## 1. The big picture

The app is a classic real-time DSP pipeline split across **four execution
contexts**, deliberately decoupled so the GUI never blocks on audio or
rendering work:

```
 ┌──────────────────┐   ring buffer    ┌─────────────────────┐   Spectrum    ┌────────────────────┐
 │  JACK RT thread  │  (lock-free SPSC) │  Spectrum thread    │  (mpsc chan)  │  Graphic thread    │
 │  (owned by JACK) │ ────samples────▶ │  spectrum::renderer  │ ───────────▶ │  graphic::renderer │
 │  audio.rs        │                  │  FFT + transforms    │ ◀───────────  │  GraphicGenerator  │
 └──────────────────┘                  │  (TICK TIMER = clock)│  empty buffer └────────────────────┘
   ┌──────────────────┐                └─────────────────────┘                      ▲
   │ JACK notif thread│                         ▲                                   │
   │ sample-rate/port │  JACK events            │  RPC: "set generator,             │  RPC: "render frame,
   │ events via Notifier                        │   apply transform config"         │   set params" (AsyncProcessor)
   └──────────────────┘                         │                                   │
 ┌──────────────────────────────────────────────┴──────────────────────────────────────┴─────────┐
 │                              GTK main thread (glib main loop)                                    │
 │   main.rs → gui::window::start                                                                   │
 │   Controllers (Rc<RefCell<…>>, source of truth) ── PubSub event bus ──▶ Views (GTK widgets)      │
 └─────────────────────────────────────────────────────────────────────────────────────────────────┘
```

**The four contexts:**

1. **GTK main thread** — the glib main loop. Runs all UI, all *controllers*, and
   all event dispatch. Everything here is single-threaded `Rc<RefCell<…>>`; it
   never touches the audio or pixel data directly.
2. **JACK real-time thread** — owned by the JACK server. Calls our
   `ProcessHandler::process` callback to hand us audio samples. Must never block
   or allocate.
3. **Spectrum rendering thread** (`spectrum::renderer`) — pulls samples from the
   ring buffer, runs the FFT, applies the spectrum transform chain.
4. **Graphic rendering thread** (`graphic::renderer`) — turns a spectrum into a
   cairo pixel buffer via the active `GraphicGenerator`.

Threads 3 and 4 each run a *single-threaded futures executor*
(`futures::executor::block_on(process_loop())`) with a `select!` loop that
multiplexes their async event sources cooperatively. They differ in one
important way:

- The **spectrum thread is the pipeline's clock.** Its `select!` has *three*
  arms — incoming buffers, RPC commands, and a **tick timer**
  (`generator.interval()`, roughly half a DFT window per the `TARGET_OVERLAP`
  ratio). The timer is what paces production of new spectra.
- The **graphic thread is purely event-driven.** Its `select!` has only *two*
  arms — incoming spectra and RPC commands; no timer. It reacts whenever a new
  `Spectrum` arrives.

So the cadence is set by the spectrum thread's tick rate; the buffer ring (§2)
just supplies backpressure so neither side runs ahead.

---

## 2. The data pipeline (audio → pixels)

Follow one sample's journey:

1. **Capture (JACK RT thread → ring buffer).** `audio.rs`:
   `AudioProcessHandler::process` copies the input port's samples (raw `f32`,
   serialized as native-endian bytes) into a JACK `RingBuffer` — a lock-free
   single-producer/single-consumer queue. *(It's wrapped in a `Mutex` only to
   satisfy a rust-jack API quirk, [rust-jack#121]; it is never actually
   contended.)*

2. **Analyze (Spectrum thread).** `spectrum/generators/audio.rs`:
   `AudioSpectrumGenerator` reads a window of samples from the ring buffer,
   applies a Hann window, runs an `rustfft` DFT, and **bins the DFT output into
   log-spaced frequency buckets** (so each octave gets equal screen space — this
   is a *music* visualizer). It keeps power (amplitude²) per Parseval, not raw
   amplitude. The result is a `Spectrum` (see `spectrum/mod.rs`).

3. **Transform (Spectrum thread).** The `Spectrum` is then pushed through an
   ordered chain of `SpectrumTransform`s — `Diffuser` (spatial smoothing),
   `VolumeNormalizer` (auto-gain), `DecibelConverter` (log scaling). See
   `spectrum/transforms/`. The order and set are driven by `Config`.

4. **Draw (Graphic thread).** `graphic/generators/spiral.rs`: a
   `GraphicGenerator` consumes the spectrum (plus a short history) and renders it
   into a `GraphicBuffer` — a raw RGB byte buffer backing a cairo `ImageSurface`.
   The result is a `Graphic`.

5. **Display (GTK thread).** The `VisualizationController` periodically asks the
   graphic thread for a finished `Graphic`, stores it, and the
   `gui::visualization` `DrawingArea` blits that cairo surface to screen.

### The buffer-recycling ring (zero-alloc steady state)

A subtle but important detail: the spectrum and graphic threads form a **closed
loop of two mpsc channels** so buffers are *recycled* instead of reallocated
every frame. Set up in `controllers/app.rs::AppController::new`:

- `spectrum_graphic` channel: Spectrum thread → Graphic thread, carries a filled
  `Spectrum`.
- `graphic_spectrum` channel: Graphic thread → Spectrum thread, hands an empty
  `SpectrumBuffer` *back* for reuse.

The graphic thread kicks things off by sending one empty buffer. The spectrum
thread fills it on each tick and sends a `Spectrum`; the graphic thread consumes
it, recovers a recyclable `SpectrumBuffer` (from history eviction), and returns
it. Exactly one buffer is in flight per direction: the spectrum thread can only
emit a new `Spectrum` on a tick *and* when it currently holds an empty buffer, so
the ring acts as backpressure and the pipeline never queues unbounded work.

> Note: step 4 (drawing pixels) is *pull-based and separate* from this loop. The
> graphic thread keeps the latest spectra in a `spectrum_history`; actual pixel
> rendering only happens when the GTK-side frame timer requests it (§4).

---

## 3. Async / threading model

There is no Tokio. Concurrency is built from three primitives:

- **`futures` mpsc/oneshot channels** — the data pipeline and RPC transport.
- **glib main loop** — the GTK thread's executor; `spawn_local` runs `!Send`
  futures on it (used to `.await` background results and update the UI).
- **`futures::executor::block_on` + `select!`** — each background thread is just
  a blocking call running one async loop that selects over its inputs.

### `AsyncProcessor<T>` — the RPC channel (`async_processor.rs`)

This is how the GTK thread *drives* a background renderer. It wraps an
`mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>`: you call `.exec(closure)` (or the
non-`&mut`, returns-a-Future `.exec_cloned`), and your closure is shipped to the
background thread, run against its owned `T` (`SpectrumRenderer` /
`GraphicRenderer`), and the return value comes back over a `oneshot` channel you
`.await`.

So "reconfigure the diffuser" or "render a frame" becomes: *send a closure to the
right thread, await the result.* `AppController` holds an
`AsyncProcessor<SpectrumRenderer>` and an `AsyncProcessor<GraphicRenderer>` and
uses them for every cross-thread command. The background thread's `select!` loop
receives these closures on its `exec_rx` arm and applies them
(`handle_exec`).

### `PubSub` — the event bus (`pubsub.rs`)

A lightweight, **type-erased publish/subscribe bus living on the GTK main loop**,
used to decouple controllers (publishers) from views (subscribers). Key traits:

- Events are any `T: Any + Send`; dispatch is keyed by `TypeId`. A subscriber for
  type `N` only ever sees events of type `N`.
- `Notifier` (the publish handle) is `Clone + Send`, so it can be called **from
  other threads** — notably the JACK notification thread. It sends events through
  a `glib::MainContext::channel`, which marshals them onto the GTK main loop;
  subscriber callbacks therefore always run on the GTK thread.
- Subscriptions are held **weakly**. `subscribe()` returns a `SubscriptionHandle`;
  when you drop it (typically in a view's `connect_destroy`), the subscription is
  automatically pruned. This is why views stash their handles and release them on
  widget destruction.

Events flowing over PubSub include `SourcePortChanged`, `InsertSpectrumTransform`
(`controllers/app.rs`), `GraphicUpdate` (`controllers/visualization.rs`), and
`InputsChanged` / `SampleRateChanged` (`source.rs`, emitted from JACK).

### Three channels, three jobs — don't conflate them

| Mechanism            | Direction / scope                        | Carries                        | Purpose                          |
|----------------------|------------------------------------------|--------------------------------|----------------------------------|
| JACK `RingBuffer`    | JACK RT thread → Spectrum thread         | raw audio samples              | hot audio capture path           |
| mpsc buffer ring     | Spectrum thread ↔ Graphic thread         | `Spectrum` / `SpectrumBuffer`  | DSP→render data pipeline         |
| `AsyncProcessor` RPC | GTK thread → a background thread (+reply) | closures `FnOnce(&mut T)`      | configure / command a renderer   |
| `PubSub`             | any thread → GTK thread (fan-out)        | `Box<dyn Any + Send>` events   | controller→view UI notifications |

---

## 4. GUI: view / controller separation

The codebase follows a deliberate MVC-ish split.

- **Controllers** (`src/controllers/`) — own application state and logic. They
  live on the GTK thread inside `Rc<RefCell<…>>`. **`AppController` is the root
  and single source of truth**: it owns the `Config`, the active JACK source, and
  the two `AsyncProcessor`s. Every other controller
  (`VisualizationController`, `ControlPaneController`, and the per-transform
  controllers like `DiffuserController`) holds a shared
  `Rc<RefCell<AppController>>`.

- **Views** (`src/gui/`) — build GTK widgets and wire signals. A view is a plain
  function (e.g. `gui::visualization::new`, `gui::control_pane::new`) that:
  1. inflates widgets from an embedded `*.ui.xml` GtkBuilder definition,
  2. connects widget signals to controller methods, and
  3. subscribes to PubSub events to refresh itself.
  Views hold **no state**; they reference controllers and pubsub handles.

- **Config** (`src/app/config.rs`) — the serializable data model
  (`AppController::config`). It is the source of truth for the views *and* the
  renderers. A UI change mutates `config`, then the controller pushes the change
  to the appropriate background renderer via an `AsyncProcessor` command. Macros
  in this file (`define_spectrum_transform_config!`,
  `define_graphic_generator_config!`) generate the enum + `create()`/`update()`
  plumbing for each pluggable transform/generator.

**Startup flow** (`gui/window.rs::start`): `block_on(AppController::new())`
(spawns the two renderer threads and the JACK client) → build the window from
`window.ui.xml` → create `VisualizationController` + view into pane 1 →
create `ControlPaneController` + view into pane 2 → `show_all()`. On window
destroy, it `spawn_local`s `AppController::shutdown()` to tear down the threads
without deadlocking the main loop.

**The render timer** (`controllers/visualization.rs`): a `gtk::timeout_add` fires
~every 40 ms (25 fps). Each tick it issues a `render` RPC to the graphic thread;
when the resulting `Graphic` returns (awaited via `spawn_local`), it's stored and
a `GraphicUpdate` event is published, prompting the `DrawingArea` to
`queue_draw`. A `RenderingState` guard skips a tick if the previous frame is
still rendering (no pile-up).

---

## 5. JACK integration specifics (`audio.rs`, `source.rs`)

- **Client setup**: `AudioSourceController::new` opens a JACK client with
  `NO_START_SERVER` (the server must already be running), registers a single
  audio **input** port, allocates the sample ring buffer, and calls
  `activate_async` with a `NotificationHandler` + `ProcessHandler`.
- **Sample path**: `ProcessHandler::process` runs on JACK's RT thread → writes
  samples into the ring buffer. That's it — all heavy lifting is downstream.
- **Events path**: `NotificationHandler` forwards JACK notifications (sample-rate
  change, port registered/unregistered/renamed) into PubSub via the `Notifier`.
  The control pane subscribes to `InputsChanged` to keep its list of connectable
  output ports current. (There's a 10 ms polling workaround in
  `controllers/control_pane.rs` for [jack2#617], where the port list isn't
  immediately consistent after an unregister notification.)
- **Wiring a source**: the user picks a JACK output port in the control pane;
  `AppController::connect_port` calls `client.connect_ports_by_name(output,
  our_input)`. The `JackSource` trait (`source.rs`) abstracts the source so a MIDI
  source could be slotted in later (only `Audio` exists today).

---

## 6. Module map

| Path                          | Responsibility                                                        |
|-------------------------------|-----------------------------------------------------------------------|
| `main.rs`                     | Entry point; creates the `gtk::Application`.                          |
| `audio.rs`                    | JACK client, RT process handler, notification handler.               |
| `source.rs`                   | `JackSource` trait, `SourceType`, JACK event types.                  |
| `async_processor.rs`          | `AsyncProcessor<T>` — closure-RPC to background threads.              |
| `pubsub.rs`                   | Type-erased, glib-backed event bus (`PubSub`, `Notifier`).           |
| `note.rs`                     | Musical note / pitch-class math (frequencies, the `note!` macro).    |
| `app/config.rs`               | `Config` data model + config-enum-generating macros.                 |
| `controllers/`                | State + logic. `app.rs` is the root `AppController`.                 |
| `gui/`                        | GTK views (`*.ui.xml` + signal wiring). Holds no state.              |
| `spectrum/`                   | `Spectrum` types, the spectrum thread, generators, transforms.       |
| `spectrum/renderer.rs`        | Spectrum background thread (`SpectrumRenderer`, `process_loop`).     |
| `spectrum/generators/audio.rs`| FFT + log-frequency binning.                                          |
| `spectrum/transforms/`        | Diffuser, VolumeNormalizer, DecibelConverter.                        |
| `graphic/`                    | `Graphic`/`GraphicBuffer` (cairo), the graphic thread, generators.   |
| `graphic/renderer.rs`         | Graphic background thread (`GraphicRenderer`, `process_loop`).       |
| `graphic/generators/spiral.rs`| The spiral visualization.                                            |
| `traits.rs`                   | `Configurable` trait (config → component).                           |
| `error.rs`                    | Crate-wide `Error` enum.                                             |

---

## 7. Mental model in one paragraph

The GTK thread holds all the **controllers** (state) and **views** (widgets),
wired together by a **PubSub** event bus. Audio enters on JACK's real-time thread
and lands in a lock-free **ring buffer**. A dedicated **spectrum thread** FFTs
those samples into a log-frequency `Spectrum` and applies a transform chain; a
dedicated **graphic thread** turns spectra into cairo pixel buffers. Those two
threads pass buffers back and forth over a pair of mpsc channels (recycling
buffers to avoid per-frame allocation). The GTK thread commands and configures
both background threads by shipping them closures over **`AsyncProcessor`** and
awaiting the replies, then blits the finished frame to a `DrawingArea` on a 25 fps
timer.

---

## 8. Running the GUI headlessly (for agents / CI)

The app is a GUI, but it can be run and visually verified with **no physical
display and no audio hardware**. Two things have to exist first, because the app
hard-requires both at startup:

1. **A display.** GTK needs an X server; `gui::window::start` also unwraps
   `gdk::Screen::default()`. A virtual framebuffer (**Xvfb**) is enough — the
   visualization is drawn with CPU cairo, so no GPU/GL is needed.
2. **A running JACK server.** `AudioSourceController::new` opens its client with
   `NO_START_SERVER` (§5), so `AppController::new()` *errors out before the window
   is ever built* if no server is up. A **dummy-backend** `jackd -d dummy` touches
   no hardware and gives the app real `system:capture_*` ports to enumerate.

`scripts/run-headless.sh` automates the whole flow — it brings up `jackd -d dummy`
and `Xvfb` (idempotently), builds, launches the app under `dbus-run-session` (so
GApplication has a session bus to register on, else it hangs), waits, screenshots
the framebuffer with ImageMagick `import`, and stops the app again:

```sh
sudo apt-get install -y jackd2 xvfb dbus-x11 imagemagick   # one-time
scripts/run-headless.sh                                    # -> /tmp/melody-shot.png
# Then view /tmp/melody-shot.png (the Read tool renders PNGs).
```

Useful env vars: `SHOT=` (output path), `WAIT=` (seconds before capture),
`KEEP_RUNNING=1` (leave the app up, e.g. to drive it / take multiple shots),
`SCREEN=` (Xvfb geometry). App stdout/stderr (`RUST_LOG=debug`) lands in
`/tmp/melody-app.log`; "`Audio source activated`" there is the signal that the
JACK layer connected. A panic at `window.rs`'s `.expect("failed to create main
window")` instead means JACK wasn't reachable (distinct from a display problem).

**What a plain run verifies — and what it doesn't.** A successful run proves GTK
rendered the full UI chrome and the JACK port list (the screenshot shows the
control pane listing `system:capture_*`). On its own it does **not** prove the
visualizer *animates*: with no signal connected the spectrum is silent, so the
spiral renders at its static base brightness.

**Driving the live pipeline.** To exercise audio → spectrum → graphic end to end,
feed the app's input a real signal with `scripts/connect-test-tone.sh`, which
starts a `jack_simple_client` sine and connects it to `Melody Visualizer:input`:

```sh
KEEP_RUNNING=1 scripts/run-headless.sh      # launch the app, leave it running
scripts/connect-test-tone.sh                # connect a sine tone
DISPLAY=:99 import -window root /tmp/tone.png
# The spiral lights one bright band at the tone's frequency (a pure sine → one
# band). `scripts/connect-test-tone.sh --disconnect` stops driving the input.
```

[rust-jack#121]: https://github.com/RustAudio/rust-jack/issues/121
[jack2#617]: https://github.com/jackaudio/jack2/issues/617
