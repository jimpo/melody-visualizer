# ARCHITECTURE

Melody Visualizer is a real-time music visualizer. It reads audio from a JACK
port, computes a log-frequency spectrum, and draws an animated graphic in a GTK 4
window.

This document is the **aspirational architecture**: the shape the code should
have. Most of it describes the system as it is today, because the current design
is sound. Where the code falls short of the intent, the gap is stated in
[§7 Target state and gaps](#7-target-state-and-gaps) instead of being hidden.

Read this before changing a component boundary, a thread, or a channel.
[DEVELOPMENT.md](DEVELOPMENT.md) covers how to build, test, and run the app.

## In one paragraph

The GTK thread holds all the controllers (state) and views (widgets), wired
together by a PubSub event bus. Audio enters on JACK's real-time thread and lands
in a lock-free ring buffer. A dedicated spectrum thread FFTs those samples into a
log-frequency `Spectrum` and applies a transform chain; a dedicated graphic
thread turns spectra into cairo pixel buffers. Those two threads pass buffers
back and forth over a pair of mpsc channels, recycling them to avoid per-frame
allocation. The GTK thread commands and configures both background threads by
shipping them closures over `AsyncProcessor` and awaiting the replies, then blits
the finished frame to a `DrawingArea` on a 25 fps timer.

---

## 1. Components

Five components, each with one job and one owner thread.

| Component | Role | Lives on |
|---|---|---|
| **Audio engine client** | Opens a JACK client, receives sample blocks on JACK's real-time thread, forwards JACK events. | JACK RT + notification threads |
| **DSP chain** | Turns a window of samples into a `Spectrum`, then applies an ordered chain of transforms. | Spectrum thread |
| **Visualizer** | Turns a `Spectrum` (plus a short history) into a pixel buffer. | Graphic thread |
| **GUI** | Builds GTK widgets, wires signals, blits the finished frame. Holds no state. | GTK main thread |
| **Controls** | Owns `Config` — the single source of truth. Translates UI actions into commands for the other components. | GTK main thread |

### Boundary contracts

Each boundary carries exactly one kind of value. Keep it that way.

- **Audio engine → DSP**: raw `f32` samples through a lock-free ring buffer, and
  nothing else. The audio client never computes anything.
- **DSP → Visualizer**: a `Spectrum` — a vector of non-negative power values plus
  the `SpectrumParams` that give each bin its frequency. The DSP never knows
  about pixels; the visualizer never knows about audio.
- **Controls → DSP and Visualizer**: a command, never shared mutable state. See
  [§4](#4-the-control-path).
- **Audio engine → Controls**: a JACK event on the source's own channel. The
  audio client knows nothing about PubSub; `AppController` republishes what it
  reads onto the bus.
- **Controls → GUI**: a typed event on the PubSub bus. Views subscribe; they are
  never called directly.

The key asymmetry: **data flows one way (audio → pixels), control flows the other
way (GUI → engine)**. The two paths use different mechanisms and never mix.

---

## 2. Concurrency model

### Execution contexts

Five contexts run concurrently. Four are threads we can reason about; one belongs
to JACK.

```
  ┌───────────────────────┐        ring buffer         ┌──────────────────────┐
  │  JACK RT thread       │      (lock-free SPSC)      │  Spectrum thread     │
  │  audio/               │ ──────── f32 samples ────▶ │  spectrum/renderer   │
  │  ProcessHandler       │                            │  FFT + transforms    │
  │  never blocks/allocs  │                            │  ▲ TICK = the clock  │
  └───────────────────────┘                            └──────────────────────┘
  ┌───────────────────────┐                             Spectrum │   ▲ SpectrumBuffer
  │  JACK notify thread   │                                      ▼   │ (recycled)
  │  NotificationHandler  │                            ┌──────────────────────┐
  └───────────┬───────────┘                            │  Graphic thread      │
              │ events                                 │  graphic/renderer    │
              │                                        │  keeps history;      │
              │                                        │  draws only on RPC   │
              │                                        └──────────────────────┘
              │                                          ▲              │
              │ PubSub                       render RPC  │              │ Graphic
              ▼                                          │              ▼
  ┌───────────────────────────────────────────────────────────────────────────┐
  │  GTK main thread (glib main loop)                                         │
  │  Controls (Rc<RefCell<…>>) ── PubSub ──▶ Views (GTK widgets)              │
  │  40 ms frame timer drives the render RPC and the DrawingArea repaint      │
  └───────────────────────────────────────────────────────────────────────────┘
```

1. **GTK main thread.** The glib main loop. Runs every controller, every view,
   and every PubSub callback. Single-threaded throughout: state is
   `Rc<RefCell<…>>`, never `Arc<Mutex<…>>`. It must never block on audio or
   pixel work.
2. **JACK real-time thread.** Owned by the JACK server, which calls
   `AudioProcessHandler::process`. It may not block, allocate, or lock. Its only
   job is to copy samples into the ring buffer.
3. **JACK notification thread.** Delivers sample-rate and port events. It only
   pushes them onto the source's event channel, which a task on the GTK thread
   drains.
4. **Spectrum thread** (`SpectrumProcessor`). Reads samples, runs the FFT, bins
   into log-spaced buckets, applies the transform chain.
5. **Graphic thread** (`GraphicProcessor`). Keeps the recent spectra and, when
   asked, draws a frame with cairo.

Threads 4 and 5 are each **one blocking call around one async loop**:
`futures::executor::block_on(process_loop())`, with a `select!` that multiplexes
that thread's inputs. There is no work-stealing runtime and no Tokio. Each
component owns its state exclusively, so nothing inside these threads is locked.

### What is actually parallel

Only the **pipeline stages** run in parallel. Nothing inside a stage does.

- Capture of block *N+1* overlaps the FFT of window *N*.
- The FFT of window *N+1* overlaps the drawing of frame *N*.
- The GTK thread stays responsive while both run.

There is **no data parallelism**: one FFT on one thread, one draw on one thread.
This is deliberate. The pipeline must meet a per-frame deadline, not maximize
throughput, and a single-owner-per-stage model removes every lock from the hot
path.

### The two clocks

The pipeline is paced by two independent timers. Do not conflate them.

| Clock | Where | Rate | Drives |
|---|---|---|---|
| **Spectrum tick** | Spectrum thread `select!` arm (`futures_timer::Delay`) | `generator.interval()` — the part of a DFT window that `audio::Config::overlap` leaves, so ~21 ms at 2048 samples / 48 kHz and half of it overlapping | How often a new `Spectrum` is produced |
| **Frame timer** | GTK thread (`glib::timeout_add_local`) | 40 ms (25 fps) | How often a frame is rendered and repainted |

The spectrum thread is the **only** timed producer. The graphic thread has no
timer at all: it reacts to arriving spectra by updating its history, and it draws
only when the GTK thread asks it to. Analysis rate and frame rate are therefore
decoupled, and either can change without the other noticing.

### Backpressure

Nothing in the pipeline queues unbounded work.

- The **buffer ring** (§3) keeps exactly one `SpectrumBuffer` in flight. The
  spectrum thread can emit only when a tick fires *and* it holds an empty buffer.
  A slow visualizer starves the next tick instead of building a backlog; the tick
  logs "skipping tick because no buffer is available" and moves on.
- The **frame timer** guards itself with a `RenderingState`. If the previous
  frame is still rendering, the tick is dropped, not queued.
- The **ring buffer** is the one place that can overrun: if the spectrum thread
  falls behind, `AudioSpectrumGenerator` skips forward to the newest window
  rather than draining stale audio. Latency is bounded; old samples are dropped.
  Should the ring fill anyway, the RT thread drops whole samples rather than
  writing part of one, so the reader's byte stream stays sample-aligned. It adds
  what it dropped to an atomic counter, which the spectrum thread reads each tick
  and logs — dropped audio would otherwise be indistinguishable from silence.

---

## 3. The data path: audio to pixels

One sample's journey:

1. **Capture** — `audio/`. `AudioProcessHandler::process` writes the input
   port's `f32` samples into a JACK `RingBuffer` (128 KiB, lock-free SPSC) as
   native-endian bytes through a `SampleWriter`, and bumps that writer's atomic
   overrun count by whatever did not fit. Nothing else happens on the RT thread.
   The matching `SampleReader` carries both ends to the spectrum thread.
2. **Analyze** — `spectrum/generators/audio.rs`. Each tick,
   `AudioSpectrumGenerator` peeks the newest window, applies a Hann window, runs
   an `rustfft` forward DFT, and **bins the output into log-spaced frequency
   buckets** so that every octave gets equal screen space. It keeps power
   (amplitude²), which Parseval's theorem preserves, and interpolates power —
   not amplitude — between adjacent bins.
3. **Transform** — `spectrum/transforms/`. The `Spectrum` passes through a
   `TransformChain`: `Diffuser` (spatial smoothing), `VolumeNormalizer`
   (auto-gain), `DecibelConverter` (log scaling). Order is position in the
   chain, and both order and membership come from `Config`. What the values
   mean at each step is *Value ranges along the chain*, below.
4. **Accumulate** — `graphic/renderer.rs`. The graphic thread pushes the spectrum
   onto a bounded `spectrum_history` whose length the active generator declares.
5. **Draw** — `graphic/generators/spiral.rs`. On a render RPC, the generator
   draws the history into a `GraphicBuffer`, a raw RGB24 byte vector backing a
   cairo `ImageSurface`. The spiral maps log-frequency to radius and pitch class
   to hue, so notes an octave apart line up on the same spoke. This is the
   pipeline's most expensive stage by two orders of magnitude, and the cost is
   pixel area rather than bin count — see
   [DEVELOPMENT.md](DEVELOPMENT.md#the-visualizer).
6. **Display** — `gui/visualization.rs`. The `DrawingArea` blits the finished
   surface. A size change resizes the buffer in place on the GTK side.

### Value ranges along the chain

Nothing in the types says what a `Spectrum`'s values mean or how large they get,
and each stage changes both. What follows is the contract as the code stands.

| Stage | Output units | Range |
|---|---|---|
| `AudioSpectrumGenerator` | power (amplitude², per Parseval) | `[0, ∞)`, unnormalized — order 0.06 for a full-scale sine, far smaller for real music |
| `Diffuser` | unchanged | unchanged: the window is normalized to sum 1, so the transform is a weighted average and preserves both total power and range |
| `VolumeNormalizer` | fraction of a running peak | nominally `[0, ~1]`, **not clamped** — a transient louder than the peak has caught up with exceeds 1 |
| `DecibelConverter` | decades above `min_level` | `[0, −log₁₀(min_level)]`, which is `[0, 6]` at the default `min_level` of 1e-6 |
| `SpiralGenerator` (consumer) | — | **assumes** `[0, 1]`: `0.2 + 0.8 * spectrum[i].min(1.0)` |

Two rules follow, and a new transform has to answer both:

1. **A transform declares whether it preserves the range or rescales it.** The
   diffuser preserves; the normalizer and the decibel converter rescale.
2. **The chain's last stage owns the output range**, because the consumer
   requires `[0, 1]` and clamps only from above. A negative value would pass
   straight into HSV; nothing emits one today and nothing forbids one either.

**The stages do not currently compose.** Only `VolumeNormalizer` produces
roughly what the consumer expects:

- The generator alone emits values around 0.06, so with no transforms the spiral
  renders nearly black.
- `DecibelConverter` emits up to 6.0. After the normalizer everything above 0.1
  saturates to white; on its own, everything above 1.0 does. This is very likely
  why `Config::default` has it commented out rather than deleted.

The fix is one of: the decibel converter normalizes to its own output range, or
the `[0, 1]` requirement moves onto the chain's output instead of living as the
consumer's private assumption. That is a decision to take now the contract is
written down, not a gap in the writing.

### Parameter bounds

| Parameter | Bound | Note |
|---|---|---|
| `Diffuser::width` | `0..10` semitones | The slider in `gui/controls/diffuser.rs` moves in semitones, so the field spans `0..10/12` octaves. Drives the O(bins²) cost — see the cliff in DEVELOPMENT.md |
| `VolumeNormalizer::rate` | `0.01..1` | Per frame. At 0 the running peak can never move, so the transform would freeze at whatever seeded it |
| `DecibelConverter::min_level` | `1e-10..1e10` | The slider is log₁₀, over `-10..10` |
| `Config::min_freq`, `max_freq` | A0 to C8 | The pitch range slider spans a piano, in semitones. The default `max_freq` of 20 kHz is above its top, so the slider opens with its upper handle on C8 while the config keeps 20 kHz until the handle moves |
| `Config::samples_per_octave` | 180, internal | The quadratic cost driver. No GUI control, and exposing it is deferred |
| `audio::Config::dft_window_size` | 2048, internal | With the overlap, sets the tick rate through `interval()` |
| `audio::Config::overlap` | `0..0.75` | The **Update rate** slider in `gui/controls/spectrum.rs` moves the overlap and reads out the rate it produces. Beyond 0.75 the rate climbs steeply for ever less new audio per tick |

### JACK client and port wiring

`AudioSource::new` opens the client with `NO_START_SERVER`, so **a JACK
server must already be running or startup fails** — `AppController::new()` errors
out before the window is ever built. The client registers exactly one audio
**input** port, allocates the ring buffer, and calls `activate_async` with the
notification and process handlers. It hands back the sample reader and an
unbounded channel of `events::Event`, both of which close when the source drops.

Nothing is connected to that port at startup. The user picks a JACK output port
in the control pane and `AppController::connect_port` asks the source to
connect it. `ControlPaneController` keeps a `GtkStringList` of those ports current by
subscribing to `PortsChanged` and re-reading `JackSource::available_inputs`,
and the pane's port list is bound to that model.

That list is correct the moment the notification arrives. JACK keeps listing a
port for a few milliseconds after announcing that it was unregistered
([jack2#617](https://github.com/jackaudio/jack2/issues/617)), so the
notification handler retires the name in `audio/ports.rs` and the enumeration
subtracts it, forgetting it again once JACK's own list agrees.

**Which port is connected is never cached.** `JackSource::connected_input`
reads it back off the port every time, and the `ports_connected` callback
reports `ConnectionChanged` whenever the graph around the input port moves.
A connection made with `jack_connect`, or by any other client, therefore shows
in the control pane exactly like one the app made itself: the port list selects
the row of whichever port JACK reports, and sends a selection back to JACK only
when it names a different port.

The `JackSource` trait (`audio/source.rs`) exists so a second source type could be
slotted in behind the same interface. Only `Audio` is implemented.

### Buffer recycling

The steady state allocates nothing per frame. Two loops recycle buffers.

**Spectrum buffers** cycle between the spectrum and graphic threads over a pair
of `futures::mpsc` channels created in `AppController::new`:

- `spectrum_graphic`: spectrum thread → graphic thread, carrying a filled
  `Spectrum`.
- `graphic_spectrum`: graphic thread → spectrum thread, returning an empty
  `SpectrumBuffer` for reuse.

The graphic thread seeds the loop with one empty buffer. Every spectrum it
evicts from its history becomes the next empty buffer it sends back. Exactly one
buffer moves in each direction, which is what makes the ring double as
backpressure.

Reconfiguration does not break the ring. The two threads are told about a new
frequency grid by separate commands, so between them the graphic thread receives
spectra still on the old grid. Those are no use as history, but
`SpectrumBuffer::regrid` puts their allocation back on the current grid and
returns it, rather than dropping it and allocating a replacement — which is what
a slider drag would otherwise cost, once per frame for as long as the drag lasts.

**Graphic buffers** cycle between the GTK thread and the graphic thread. The
`VisualizationController` holds the previous frame's buffer while idle, ships it
inside the render closure, and receives the drawn `Graphic` back.

`SpectrumParams` is shared as an `Arc` and compared with `Arc::ptr_eq`. A pointer
mismatch means the parameters changed, so caches (the diffuser window, the
spiral's precomputed edges, the graphic thread's history) rebuild themselves.
This is how a parameter change propagates without an explicit invalidation
message.

On the spectrum side that comparison happens once per frame, in
`TransformChain::set_params`, which hands the new grid to every transform in the
chain — and to a transform the moment it joins one. `SpectrumTransform::set_params`
is the hook, and it defaults to doing nothing, for a transform whose output
depends only on the values it is given.

The graphic side works the same way, with a second hook for the other thing a
generator derives geometry from. `GraphicRenderer` owns the grid and sees every
buffer, so it is what calls `GraphicGenerator::set_params` when the grid changes
and `set_size` when a differently-sized buffer arrives — and both when
`update_generator` swaps in a generator that has been told neither.
`GraphicGenerator::generate` therefore draws and nothing else: it compares no
state and rebuilds no cache.

---

## 4. The control path

### Config is the source of truth

`app/config.rs::Config` holds every user-visible setting: frequency range,
samples per octave, the generator choice, the transform chain, the graphic
generator's parameters. It lives on the GTK thread inside `AppController`.

The background threads do **not** hold a copy of `Config`. They hold *live
objects* built from it — `Box<dyn SpectrumTransform>`, `Box<dyn
GraphicGenerator>`. A UI change follows one path every time:

```
widget signal → controller mutates Config → controller ships a closure
              → background thread applies it to its live object
```

The `Configurable` trait (`new(config)` / `set_config(config)`) and the
`define_*_config!` macros generate the plumbing. `update()` downcasts the live
object: if it already has the right concrete type it is reconfigured in place,
otherwise it is replaced. This keeps a slider drag from reallocating a transform
on every frame.

### Each mechanism and its job

| Mechanism | Direction | Carries | Purpose |
|---|---|---|---|
| JACK `RingBuffer` | RT thread → spectrum thread | `f32` samples | Hot audio capture |
| `async_channel` | JACK notification thread → GTK thread | `audio::source::events::Event` | Report what JACK did to the source |
| `mpsc` buffer ring | Spectrum thread ↔ graphic thread | `Spectrum` / `SpectrumBuffer` | DSP → render data path |
| `AsyncProcessor` RPC | GTK thread → a background thread, with reply | `Box<dyn FnOnce(&mut T) + Send>` | Command and configure a renderer |
| `PubSub` | Any thread → GTK thread, fan-out | `Box<dyn Any + Send>` | Notify views that state changed |

**`AsyncProcessor<T>`** (`async_processor.rs`) is the whole cross-thread command
surface. `exec(closure)` sends a boxed `FnOnce(&mut T)` to the thread that owns
`T` and awaits the return value over a `oneshot`. "Reconfigure the diffuser" and
"render a frame" are both just closures. Because the closure runs on the owning
thread, there is no lock and no shared mutable state — the renderer's `&mut self`
is genuinely exclusive.

`stop()` closes the command channel, which is what ends a `process_loop`. Closing
any of a thread's input channels terminates it cleanly. The close applies to
every clone of the `AsyncProcessor`, so a renderer stops even though other
handles to it — the sample-rate subscription, say — are still alive.

**`PubSub`** (`pubsub.rs`) is a type-erased event bus on the GTK main loop.
Dispatch is keyed by `TypeId`, so a subscriber for `N` only ever sees `N`.
`Notifier` is `Clone + Send` and pushes into an unbounded `async_channel`, so
any thread can publish without blocking; a task on the main context drains it,
which is why every subscriber callback runs on the GTK thread. Subscriptions are held **weakly**: `subscribe()` returns a
`SubscriptionHandle`, and dropping it (typically in a view's `connect_destroy`)
prunes the subscription. This is why views stash their handles.

Events today: `PortsChanged`, `ConnectionChanged`, `SampleRateChanged` and
`ServerShutdown` (from JACK), `ConfigChanged` (from `AppController`),
`GraphicUpdate` (from `VisualizationController`).

`ConfigChanged` is the one a view subscribes to in order to report a config
value it does not own the control for. `AppController` publishes it from every
method that hands a config change to a background thread, so it covers whichever
control moved.

The JACK four do not reach the bus from the audio module. `AudioSource::new`
returns a channel of `events::Event`, and `AppController` spawns one task on the
main context that drains it and republishes each event under its own type. That
task is the only place the audio module and PubSub meet, which is what keeps
`src/audio/` free of any GUI dependency.

### Controllers and views

- **Controllers** (`src/controllers/`) own state and logic. `AppController` is
  the root: it owns `Config`, the JACK source, and both `AsyncProcessor`s. Every
  other controller holds an `Rc<RefCell<AppController>>`.
- **Views** (`src/gui/`) are plain functions that inflate a `*.ui.xml`
  GtkBuilder definition, connect signals to controller methods, and subscribe to
  PubSub events. **A view holds no state.** If a view needs to remember
  something, that something belongs in a controller.

### Lifecycle

**Startup** (`gui/window.rs::start`): `block_on(AppController::new())` spawns
both renderer threads and the JACK client, then pushes the initial config to
them. The window is built, the two panes are populated, and CSS is applied.

The window is a header bar over a `GtkPaned`: the visualization on the left, the
control pane on the right at a fixed width. Only the visualization resizes with
the window, and the pane collapses by being hidden rather than by narrowing, so
the visualization is never covered. Fullscreen takes the header bar away, and
the pane toggle it carries moves to a button floating over the canvas that
withdraws once the pointer holds still.

The control pane is an accordion over the pipeline, one stage per step: the
source, the spectrum generator, each transform in chain order, then the graphic
generator. A stage is a bar over a `GtkRevealer`, at most one is open, and every
bar reports what its stage is set to whether it is open or not. The chain the
pane shows is fixed — nothing in the GUI adds, removes or reorders a
transform. A transform that can be done without carries a switch on its row
that means *enabled*: off dims the row and makes the body insensitive, and the
row goes on reporting what the stage is set to.

The stage bodies are built from a small widget vocabulary in `gui/controls/`:
a captioned slider for each transform, a boxed list for the ports, a row of
twelve keys for the spiral's key, and a two-handle slider for its pitch range.
GTK 4 has no two-handle scale, so that last one is a `GtkDrawingArea` over two
`GtkAdjustment`s that clamp each other, drawn with cairo and driven by a drag
gesture.

**Shutdown**: `connect_destroy` calls `AppController::shutdown()`, which drops
the JACK source and closes each renderer's command channel. Closing a channel
ends that renderer's loop and its thread winds down on its own; the call returns
at once, so the main loop is never blocked.

---

## 5. Module map

| Path | Responsibility |
|---|---|
| `lib.rs` | Library root; declares the public module tree. |
| `main.rs` | Binary entry point; creates the `gtk::Application` and calls `gui::window::start`. |
| `audio/mod.rs` | JACK client, RT process handler, notification handler. |
| `audio/ports.rs` | Port enumeration, and the jack2#617 settling workaround. |
| `audio/ring.rs` | Both ends of the capture ring, and the overrun count they share. |
| `audio/source.rs` | `JackSource` trait, `SourceType`, JACK event types. |
| `async_processor.rs` | `AsyncProcessor<T>` — closure RPC to a background thread. |
| `pubsub.rs` | Type-erased event bus (`PubSub`, `Notifier`). |
| `note.rs` | Musical note and pitch-class math; the `note!` macro. |
| `app/config.rs` | `Config` plus the config-enum macros. |
| `controllers/` | State and logic; `app.rs` is the root. |
| `gui/` | GTK views (`*.ui.xml` + signal wiring). Stateless. |
| `spectrum/` | `Spectrum` types, spectrum thread, generators, transforms. |
| `graphic/` | `Graphic`/`GraphicBuffer` (cairo), graphic thread, generators. |
| `traits.rs` | `Configurable` — build or update a component from its config. |
| `error.rs` | Crate-wide `Error`. |
| `test_support/` | Shared fixtures: the glib main-loop driver, synthesized audio, the wiring that builds a renderer from a `Config`, and a private JACK server (`jackd.rs`). Public under the `testing` feature. |

---

## 6. Invariants

Rules that keep the design intact. Breaking one needs a note in this document.

1. **The JACK RT thread does not block, allocate, or lock.** It copies samples
   and returns.
2. **Each pipeline stage has exactly one owner thread.** State crosses a thread
   boundary by moving through a channel, never by being shared.
3. **The GTK thread never blocks on background work.** It sends a command and
   awaits it with `spawn_local`.
4. **`Config` is the only source of truth.** A background thread's live objects
   are derived from it and are never read back as authority.
5. **Views hold no state and subscribe weakly.**
6. **The data path never queues.** Every stage drops or stalls instead of
   building a backlog.
7. **Data flows forward, control flows back.** The DSP never reaches into the
   GUI; the visualizer never reaches into the DSP.
8. **A cairo surface never outlives the call it was made for.** `GraphicBuffer`
   lends a transient surface over pixels it owns; a clone that survives the
   callback aliases a buffer the pipeline goes on writing to.

---

## 7. Target state and gaps

Where the code does not yet match the architecture above. Each item is a
candidate for its own change.

### Audio engine client

- **The sample seam names JACK.** `AudioSpectrumGenerator` asks `SampleReader`
  for `f32` windows and no longer sees a byte, but `SampleReader` is a concrete
  JACK ring. Ports and connections are behind `JackSource`; the target is an
  **audio-engine-agnostic input port**: a trait that yields `f32` frames as well
  as a device/port list, with the JACK client as one implementation. That is
  what makes PipeWire, ALSA, or a file source possible.
- JACK sample-rate changes travel through the app-wide event bus to
  `AppController`, which sends them over the spectrum processor's control
  channel. The audio generator updates both its frequency mapping and tick
  interval without rebuilding the rate-independent log-frequency grid.
- **`SourceType::MIDI` exists but nothing implements it.** Either build the MIDI
  source or drop the variant.
- **A workaround is load-bearing**: unregistered port names are retired inside
  `audio/ports.rs` because JACK keeps listing them (jack2#617). It should be
  revisited against current upstream.
- **JACK server shutdown is reported but not acted on** — the source publishes
  `ServerShutdown` and nothing subscribes. The app should tell the user and stop
  the pipeline.

### DSP chain

- **The stages do not compose over their value ranges.** `DecibelConverter` emits
  up to 6.0 into a consumer that assumes `[0, 1]`, which is why it is commented
  out of `Config::default`. See *Value ranges along the chain* above for the two
  ways out.
- **The transforms measure time in ticks, and the tick is adjustable.**
  `VolumeNormalizer::rate` is per spectrum, so raising the update rate shortens
  the time its running peak decays over without its own control moving. A rate
  in seconds, scaled by `generator.interval()`, would decouple them.
- `TransformChain::remove` and `reorder` exist, but nothing in the GUI calls
  them: transforms can still only be appended.
- **The switch on a transform row reaches nothing.** `TransformChain` can bypass
  an entry, but `Config` has no field for it and nothing carries the switch's
  state to the chain, so switching a transform off dims its row and changes
  nothing about the spectrum.
- The spectrum thread's own loop — timing, backpressure, shutdown — has no
  tests. The chain it drives is covered without one, since `SpectrumRenderer`
  is drivable on its own, but `SpectrumProcessor` is reachable only by spawning
  a thread.

### Visualizer

- `SpiralGenerator` is the only generator, and `history_len()` is hardcoded to 1,
  so the history mechanism is never exercised. Either use it or simplify it away.
- `GraphicBuffer::with_image_surface` extends a slice's lifetime with
  `mem::transmute` to satisfy `ImageSurface::create_for_data`. It is the one
  piece of `unsafe` in the crate. The construction that would retire it —
  holding an `ImageSurface` in the buffer — is ruled out by `Graphic` having to
  be `Send`, so the pixels travel as a `Vec<u8>` and a surface is built around
  them per call. A caller that lets a surface clone escape gets
  `Error::GraphicDrawClonesSurface` and leaks that buffer's allocation.

### GUI

- **Frame timing ignores the compositor.** A fixed 40 ms `glib::timeout_add_local`
  should become GTK 4's frame clock (`add_tick_callback`), which aligns repaints
  with vsync and reports the real frame deadline.
- Errors from the renderer threads are logged, not surfaced. `error_dialog`
  exists but the pipeline does not use it.

### Controls

- **`Config` does not persist.** There is no serde derive, no load, no save. The
  app starts from `Config::default()` every time. Persisting the config — and
  named presets — is the largest single user-facing gap.
- `Config::default()` documents `min_freq: 200.0` as "low-end of human hearing",
  which is wrong; the value is a visualization choice, not a hearing limit.
- Config changes reach the renderers as several independent RPCs
  (`sync_spectrum_transforms`, `update_spectrum_params`,
  `update_spectrum_generator`, `update_graphic_generator`). There is no single
  "apply this config" path, so a new setting is easy to forget to wire up.

### Cross-cutting

- **`cargo test` aborts**, even though every test passes. The glib main-loop
  tests share the process-global default `MainContext`, so a second test in the
  same process trips glib's thread guard and the process takes a non-unwinding
  panic. See [DEVELOPMENT.md](DEVELOPMENT.md#testing) for the workaround and the
  fix this needs.
- The crate builds clean under `cargo clippy --all-targets -- -D warnings`.
