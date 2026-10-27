# ARCHITECTURE

Melody Visualizer is a real-time music visualizer. It is a CLAP plugin, and a
standalone JACK app from the same code, both built on
[nice-plug](https://codeberg.org/RustAudio/nice-plug). It takes audio from the
host, computes a log-frequency spectrum, and draws an animated spiral in an
[iced](https://iced.rs/) window.

This document is the **aspirational architecture**: the shape the code should
have. Most of it describes the system as it is today. Where the code falls
short of the intent, the gap is stated in
[§7 Target state and gaps](#7-target-state-and-gaps) instead of being hidden.

Read this before changing a component boundary, a thread, or a channel.
[DEVELOPMENT.md](DEVELOPMENT.md) covers how to build, test, and run the app.

## In one paragraph

The host calls the plugin's `process` on its audio thread, which mixes the
input to mono and writes it into a lock-free ring. A dedicated spectrum thread
FFTs those samples into a log-frequency `Spectrum` and applies a transform
chain; a dedicated graphic thread turns spectra into pixel buffers. Those two
threads pass buffers back and forth over a pair of mpsc channels, recycling
them to avoid per-frame allocation. Together they are the `Engine`, which runs
while the host has the plugin active. The editor runs on the plugin window's
thread: each time `process` wakes it, it turns the plugin's parameters into a
`Config`, applies what changed to the engine, has the engine draw a frame the
size of the window, and uploads the frame to the GPU.

---

## 1. Components

Five components, each with one job and one owner thread.

| Component | Role | Lives on |
|---|---|---|
| **Plugin** (`lib.rs`) | nice-plug's `Plugin`. Starts and stops the engine with the host's activation, and feeds it audio. | Host's main thread, and its audio thread for `process` |
| **Parameters** (`params.rs`) | Every setting, as a host parameter. The single source of truth. | Shared: the host, the editor and the plugin read them |
| **DSP chain** | Turns a window of samples into a `Spectrum`, then applies an ordered chain of transforms. | Spectrum thread |
| **Visualizer** | Turns a `Spectrum` (plus a short history) into a pixel buffer. | Graphic thread |
| **Editor** (`editor.rs`, `editor/`) | The window: the spiral, the interval ring's labels, and the control pane. Sets parameters through the host; applies them to the engine. | The window's thread (baseview's event loop) |

The DSP chain and the visualizer together are the **engine**
(`engine.rs`), which the plugin owns while active.

### Boundary contracts

Each boundary carries exactly one kind of value. Keep it that way.

- **Plugin → DSP**: mono `f32` samples through a lock-free ring buffer, and
  nothing else. `process` never computes anything beyond the mono mix.
- **DSP → Visualizer**: a `Spectrum` — a vector of non-negative amplitude
  values plus the `SpectrumParams` that give each bin its frequency. The DSP
  never knows about pixels; the visualizer never knows about audio.
- **Editor → engine**: a `Config` applied through `Engine::apply`, which ships
  commands to the threads — never shared mutable state. See
  [§4](#4-the-control-path).
- **Editor → host**: a parameter change, as a begin/set/end gesture. The
  editor never writes a parameter's value directly.
- **Plugin → editor**: a wake-up, nothing more. `process` notifies the
  editor that there is new audio to draw.

The key asymmetry: **data flows one way (audio → pixels), control flows the
other way (parameters → engine)**. The two paths use different mechanisms and
never mix.

---

## 2. Concurrency model

### Execution contexts

```
  ┌───────────────────────┐        ring buffer         ┌──────────────────────┐
  │  Host audio thread    │    (rtrb, lock-free SPSC)  │  Spectrum thread     │
  │  Plugin::process      │ ──── mono f32 samples ───▶ │  spectrum/renderer   │
  │  never blocks/allocs  │                            │  FFT + transforms    │
  └───────────┬───────────┘                            │  ▲ TICK = the clock  │
              │ notify                                 └──────────────────────┘
              │ (PollSubNotifier)                       Spectrum │   ▲ SpectrumBuffer
              │                                                  ▼   │ (recycled)
              │                                        ┌──────────────────────┐
              │                                        │  Graphic thread      │
              │                                        │  graphic/renderer    │
              │                                        │  keeps history;      │
              │                                        │  draws only on RPC   │
              │                                        └──────────────────────┘
              │                              apply/render  ▲            │
              │                                  RPC       │            │ Graphic
              ▼                                            │            ▼
  ┌───────────────────────────────────────────────────────────────────────────┐
  │  Editor thread (baseview event loop, iced)                                │
  │  params → Config → Engine::apply → Engine::render → GPU upload            │
  └───────────────────────────────────────────────────────────────────────────┘
```

1. **Host audio thread.** Calls `Plugin::process`. It may not block,
   allocate, or lock; the `assert_process_allocs` feature makes a debug build
   abort if it allocates. It mixes the input to mono into a buffer allocated in
   `activate`, writes it into the ring, and wakes the editor if it is open.
2. **Host main thread.** Calls `activate`, `deactivate` and `editor`. It
   starts and stops the engine.
3. **Spectrum thread** (`SpectrumProcessor`). Reads samples, runs the FFT,
   bins into log-spaced buckets, applies the transform chain.
4. **Graphic thread** (`GraphicProcessor`). Keeps the recent spectra and, when
   asked, draws a frame.
5. **Editor thread.** baseview's event loop, running iced. Exists only while
   the window is open.

Threads 3 and 4 are each **one blocking call around one async loop**:
`futures::executor::block_on(process_loop())`, with a `select!` that
multiplexes that thread's inputs. There is no work-stealing runtime and no
Tokio. Each component owns its state exclusively, so nothing inside these
threads is locked.

The one lock is `Arc<Mutex<Option<Engine>>>`, shared by the plugin and the
editor. The main thread takes it to swap the engine in and out; the editor
takes it to apply and render. The audio thread never touches it: it holds the
ring's writer on its own.

### What is actually parallel

Only the **pipeline stages** run in parallel. Nothing inside a stage does.

- Capture of block *N+1* overlaps the FFT of window *N*.
- The FFT of window *N+1* overlaps the drawing of frame *N*.
- The GPU upload of frame *N* overlaps everything else.

There is **no data parallelism**: one FFT on one thread, one draw on one
thread. This is deliberate. The pipeline must meet a per-frame deadline, not
maximize throughput, and a single-owner-per-stage model removes every lock from
the hot path.

### The two clocks

The pipeline is paced by two independent clocks. Do not conflate them.

| Clock | Where | Rate | Drives |
|---|---|---|---|
| **Spectrum tick** | Spectrum thread `select!` arm (`futures_timer::Delay`) | `generator.interval()` — one over the **Update rate** parameter, so 20 ms at the default 50 / sec, whatever the window | How often a new `Spectrum` is produced |
| **Frame poll** | nice-plug-iced's `poll_events` subscription | At most 64 / sec, and only after `process` has notified since the last poll | How often a frame is rendered and shown |

The spectrum thread is the **only** timed producer. The graphic thread has no
timer at all: it reacts to arriving spectra by updating its history, and it
draws only when the editor asks it to. Analysis rate and frame rate are
therefore decoupled. Because frames follow `process`, the spiral stops moving
whenever the host stops processing the plugin.

### Backpressure

Nothing in the pipeline queues unbounded work.

- The **buffer ring** (§3) keeps exactly one `SpectrumBuffer` in flight. The
  spectrum thread can emit only when a tick fires *and* it holds an empty
  buffer. A slow visualizer starves the next tick instead of building a
  backlog; the tick logs "skipping tick because no buffer is available" and
  moves on.
- The **editor** draws no new frame while the last one is still uploading to
  the GPU, so a frame poll that arrives meanwhile is dropped, not queued.
- The **capture ring** is the one place that can overrun: if the spectrum
  thread falls behind, `AudioSpectrumGenerator` skips forward to the newest
  window rather than draining stale audio. Latency is bounded; old samples are
  dropped. Should the ring fill anyway, `process` drops what does not fit and
  adds it to an atomic counter, which the spectrum thread reads each tick and
  logs — dropped audio would otherwise be indistinguishable from silence.

---

## 3. The data path: audio to pixels

One sample's journey:

1. **Capture** — `lib.rs`, `audio/ring.rs`. `process` mixes the host's
   channels to mono, averaging them, and `SampleWriter::write_samples` pushes
   the result into an [rtrb](https://crates.io/crates/rtrb) ring sized for the
   longest window plus the slowest hop at the active sample rate. Nothing else
   happens on the audio thread.
2. **Analyze** — `spectrum/generators/audio.rs`. Each tick,
   `AudioSpectrumGenerator` peeks the newest window, applies a Hann window,
   runs an `rustfft` forward DFT, and **bins the output into log-spaced
   frequency buckets** so that every octave gets equal screen space. A read
   shorter than the window, which happens while the ring refills after the
   window grows, is padded with silence. It bins power (amplitude²), which
   Parseval's theorem preserves and which adds across incoherent components,
   and interpolates power — not amplitude. Each DFT bin spreads its power as a
   triangle in log frequency, peaking at that bin and reaching zero at its two
   neighbours, so the grid stays filled at the low end, where it is tens of
   bins to a DFT bin, and the triangle collapses to a linear split between the
   two nearest bins at the top, where the grid is the coarser of the two. **A
   bin sums power, not power per octave.** The square root comes last, once
   per bin, so the generator emits amplitude and every transform after it
   works in that unit: a partial at half the amplitude of another is drawn at
   half its value, not a quarter.
   Spreading a tone over the ±1 DFT bin the window resolves keeps the total but
   lowers the peak, and it lowers it most at the bottom of the range where that
   width is widest — so a bass note is drawn dimmer than a treble note of the
   same energy. Giving the bass real resolution rather than a wide hump is what
   a longer window buys, not a change to the binning.
3. **Transform** — `spectrum/transforms/`. The `Spectrum` passes through a
   `TransformChain`: `HarmonicSummation` (pitch salience), `PowerMap`
   (contrast), `Diffuser` (spatial smoothing), `VolumeNormalizer` (auto-gain).
   A fifth, `DecibelConverter` (log scaling), exists but is not in the default
   chain. The harmonic summation scores each bin by its own value plus a
   weight of `decay^(h−1)` of the value at `h` times its frequency, for `h` up
   to `harmonics`. The fundamental collects its whole series and a harmonic
   collects only its own sparse one, so the note, not its overtones, lights up
   brightest. On the log grid a multiple of frequency is a constant index
   shift, `round(bins_per_octave · log2(h))`, derived in `set_params`. The
   power map raises every bin to `exponent`. Values end up relative to the
   running peak the normalizer sets, so a bin at a ratio `r` of the peak shows
   at `r^exponent`: 2 gives the contrast of drawing power, 1 is amplitude, and
   below 1 compresses. It follows the summation so the sum adds amplitudes
   rather than letting the loudest partial dominate, and precedes the diffuser
   so the sharpened peak is what gets spread. Order is position in the chain,
   and both order and membership come from `Config::default`. What the values
   mean at each step is *Value ranges along the chain*, below.
4. **Accumulate** — `graphic/renderer.rs`. The graphic thread pushes the
   spectrum onto a bounded `spectrum_history` whose length the active
   generator declares.
5. **Draw** — `graphic/generators/spiral.rs`. On a render RPC, the generator
   writes the history into a `GraphicBuffer`, plain RGBA bytes. The spiral
   maps log-frequency to radius and pitch class to hue, so notes an octave
   apart line up on the same spoke. It writes every pixel from a map of which
   two neighbouring bins light that pixel, in what blend, and how brightly,
   rebuilt when the size, grid or config changes. This is the pipeline's most
   expensive stage by an order of magnitude, and the cost is pixel area rather
   than bin count — see [DEVELOPMENT.md](DEVELOPMENT.md#the-visualizer).
6. **Display** — `editor.rs`. The editor hands the pixels to iced as an image
   and uploads them with `image::allocate`. iced uploads an image this large
   off the render thread and draws nothing for it until the upload lands, so
   the editor keeps the previous frame's allocation on show until the new one
   is ready. The interval ring's labels are text, drawn by an iced canvas
   stacked over the image at the radius `spiral::outer_radius` reports.

The frame is drawn in physical pixels at the size of the window's room beside
the control pane, so it shows one to one at any display scale.

### Value ranges along the chain

Nothing in the types says what a `Spectrum`'s values mean or how large they
get, and each stage changes both. What follows is the contract as the code
stands.

| Stage | Output units | Range |
|---|---|---|
| `AudioSpectrumGenerator` | amplitude (√ of the power binned per Parseval) | `[0, ∞)`, unnormalized — order 0.06 for a full-scale sine, far smaller for real music |
| `HarmonicSummation` | amplitude, weighted sum | rescales: `[0, max · Σ decay^(h−1)]`, up to about 4.2× the input's peak at the defaults. A partial whose harmonics fall off the top of the grid keeps only its own value |
| `PowerMap` | amplitude raised to `exponent` | rescales: `[0, max^exponent]`, saturating at `f64::MAX` rather than overflowing |
| `Diffuser` | unchanged | unchanged: the window is normalized to sum 1, so the transform is a weighted average and preserves both the values' sum and range. Spreading amplitude linearly spreads less power than spreading power would, so a narrow peak loses more of its power than a wide hump |
| `VolumeNormalizer` | fraction of a running peak | nominally `[0, ~1]`, **not clamped** — a transient louder than the peak has caught up with exceeds 1 |
| `DecibelConverter` | decades above `min_level` | `[0, −log₁₀(min_level)]`, which is `[0, 3]` at a `min_level` of 1e-3 (−60 dB in amplitude) |
| `SpiralGenerator` (consumer) | — | **expects** `[0, ~1]` with 1 as the peak, **not clamped**: `REST + (1 − REST) · exposure · spectrum[i]` in linear light. Light past 1 overexposes: the hue's brightest channel stays full and the colour moves toward white, reaching it at 2 |

Two rules follow, and a new transform has to answer both:

1. **A transform declares whether it preserves the range or rescales it.** The
   diffuser preserves; the harmonic summation, the power map, the normalizer
   and the decibel converter rescale.
2. **The chain's last stage owns the output range**, because the consumer
   treats 1 as the peak and draws anything above it as overexposure toward
   white.

This is why the volume normalizer has no on/off parameter: without it the
generator's values, around 0.06, leave the spiral nearly black.

### Parameter bounds

Each parameter is declared in `params.rs`; its default is read out of
`Config::default()` so the two cannot drift apart.

| Parameter | Bound | Note |
|---|---|---|
| Window | `10..1000` ms | The generator sizes the DFT to the milliseconds times the sample rate. The ring is sized for `audio::MAX_WINDOW_MS` |
| Update rate | `1..100` / sec | Sets the tick through `interval()`, independent of the window; a hop longer than the window skips audio. The ring is sized for the hop at `audio::MIN_UPDATE_RATE` |
| Harmonics | `1..16` | The fundamental counts as one, so 1 is the identity. Cost is O(bins × harmonics) |
| Harmonic decay | `0..1` | The weight of each harmonic against the one below it. 0 is the identity |
| Exponent | `0.5..4`, step 0.1 | The power map's. 1 is the identity |
| Diffuser width | `0..10` semitones | The config field is in octaves. Drives the O(bins²) cost — see the cliff in DEVELOPMENT.md |
| Normalizer rate | `0.01..1` | Per spectrum. At 0 the running peak could never move |
| Lowest note, Highest note | A0 to C8 | Semitones above A0, shown as note names. The default range is D2 to D6. The editor's range slider keeps the lowest at or below the highest; the host can still set them crossed, and `PluginParams::pitch_range` reads them lowest first |
| Key | C to B | The pitch class at the top of the spiral |
| Exposure, Centre hole, Outer padding, Glow core, Glow falloff | see `params.rs` | The spiral's look. The glow is set in cents and stored in octaves |

`Config::samples_per_octave`, 180, is not a parameter: it is the density of
the display grid the DFT output is folded onto, not a resolution, and it is
the quadratic cost driver — see the cliff in DEVELOPMENT.md.

### The host and its audio

The host owns audio routing. The plugin declares a stereo and a mono layout
and analyzes the main input whatever its width; there is no port to choose.
In the standalone build, nice-plug's JACK backend plays the host, and its
`--connect-jack-inputs` option is the routing.

The sample rate comes from `activate`. A host changes it by deactivating and
reactivating the plugin, which starts a fresh engine at the new rate.

### Buffer recycling

**Spectrum buffers** cycle between the spectrum and graphic threads over a
pair of `futures::mpsc` channels created in `Engine::start`:

- `spectrum_graphic`: spectrum thread → graphic thread, carrying a filled
  `Spectrum`.
- `graphic_spectrum`: graphic thread → spectrum thread, returning an empty
  `SpectrumBuffer` for reuse.

The graphic thread seeds the loop with one empty buffer. Every spectrum it
evicts from its history becomes the next empty buffer it sends back. Exactly
one buffer moves in each direction, which is what makes the ring double as
backpressure.

Reconfiguration does not break the ring. The two threads are told about a new
frequency grid by separate commands, so between them the graphic thread
receives spectra still on the old grid. Those are no use as history, but
`SpectrumBuffer::regrid` puts their allocation back on the current grid and
returns it, rather than dropping it and allocating a replacement — which is
what a slider drag would otherwise cost, once per frame for as long as the
drag lasts.

**Graphic buffers** are not recycled: `Engine::render` allocates a frame's
pixels each call, because they go on to iced rather than back. See §7.

`SpectrumParams` is shared as an `Arc` and compared with `Arc::ptr_eq`. A
pointer mismatch means the parameters changed, so caches (the diffuser window,
the harmonic shifts, the spiral's pixel map, the graphic thread's history)
rebuild themselves. This is how a parameter change propagates without an
explicit invalidation message.

On the spectrum side that comparison happens once per frame, in
`TransformChain::set_params`, which hands the new grid to every transform in
the chain — and to a transform the moment it joins one.
`SpectrumTransform::set_params` is the hook, and it defaults to doing nothing,
for a transform whose output depends only on the values it is given.

The graphic side works the same way, with a second hook for the other thing a
generator derives geometry from. `GraphicRenderer` owns the grid and sees every
buffer, so it is what calls `GraphicGenerator::set_params` when the grid
changes and `set_size` when a differently-sized buffer arrives — and both when
`update_generator` swaps in a generator that has been told neither.
`GraphicGenerator::generate` therefore compares no state. The spiral's cache is
the one that is dear to rebuild, a pass over every pixel, so its hooks only
mark its pixel map stale and `generate` rebuilds it before the next frame: a
slider drag that reconfigures it many times a frame rebuilds it once a frame.

---

## 4. The control path

### The parameters are the source of truth

`PluginParams` holds every user-visible setting as a nice-plug parameter. The
host reads, saves, restores and automates them; the editor's controls show
and set them. `PluginParams::config` is the one place they become the engine's
`Config`.

The background threads do **not** hold a copy of the parameters. They hold
*live objects* built from a `Config` — `Box<dyn SpectrumTransform>`,
`Box<dyn GraphicGenerator>`. A change follows one path every time, whether it
came from the editor or from host automation:

```
parameter changes → next frame: params.config() → Engine::apply(config)
                  → engine diffs against the config it last applied
                  → ships a closure per changed piece
                  → the owning thread applies it to its live object
```

`Engine::apply` reconfigures a transform in place rather than rebuilding it, so
a stage that keeps state across frames, like the volume normalizer's running
peak, keeps it while one of its settings moves. The `Configurable` trait
(`new(config)` / `set_config(config)`) and the `define_*_config!` macros
generate the plumbing. `update()` downcasts the live object: if it already has
the right concrete type it is reconfigured in place, otherwise it is replaced.

Changes are applied only when the editor draws. With the window closed, the
engine keeps whatever it last applied, and the next `activate` starts it from
the parameters as they are.

### Each mechanism and its job

| Mechanism | Direction | Carries | Purpose |
|---|---|---|---|
| rtrb ring (`audio/ring.rs`) | Audio thread → spectrum thread | `f32` samples | Hot audio capture |
| `mpsc` buffer ring | Spectrum thread ↔ graphic thread | `Spectrum` / `SpectrumBuffer` | DSP → render data path |
| `AsyncProcessor` RPC | Editor thread → a background thread, with reply | `Box<dyn FnOnce(&mut T) + Send>` | Configure a renderer, or render a frame |
| `PollSubNotifier` | Audio thread → editor thread | a flag | Wake the editor to draw |
| nice-plug parameters | Editor and host → everyone | parameter values | The settings |

**`AsyncProcessor<T>`** (`async_processor.rs`) is the whole cross-thread
command surface. `exec(closure)` sends a boxed `FnOnce(&mut T)` to the thread
that owns `T` and returns a future of the closure's result over a `oneshot`.
"Reconfigure the diffuser" and "render a frame" are both just closures.
Because the closure runs on the owning thread, there is no lock and no shared
mutable state — the renderer's `&mut self` is genuinely exclusive. The engine
waits on each with `block_on`: each thread answers between two ticks or two
frames, so the wait is short.

`stop()` closes the command channel, which is what ends a `process_loop`.
Closing any of a thread's input channels terminates it cleanly. Dropping the
`Engine` closes them all.

### The editor

`editor.rs` is the iced application: the spiral on the left and the control
pane (`editor/pane.rs`) at a fixed width on the right. Only the spiral resizes
with the window. The editor's own state is what the parameters do not hold:
the frame on show, which stage is open, and which slider is mid-drag.

The pane is an accordion over the pipeline, one stage per row: the spectrum
generator, each transform in chain order, then the spiral. At most one stage is
open, and every row reports what its stage is set to whether it is open or
not. The chain is fixed — nothing in the editor adds, removes or reorders a
transform. A transform that can be done without carries a switch on its row,
a `BoolParam` that bypasses the stage in the running chain: the harmonic
summation, the power map and the diffuser qualify.

The spectrum stage exposes two sliders, both in units a listener can read
without the sample rate: the **Window** in milliseconds, which is the audio
each spectrum analyses and so both its frequency resolution and its latency,
and the **Update rate** in spectra per second.

Each control reads its parameter on every redraw, so host automation moves the
controls. A slider drag is one gesture to the host: `raw_begin_set_parameter`
on the first move, `raw_end_set_parameter` on release. A switch or a key is a
begin/set/end in one go. The pitch range is a two-handle slider drawn on an
iced canvas (`editor/range_slider.rs`), since iced has none; its handles clamp
each other.

### Lifecycle

- **`activate`** stops any engine already running (a host may activate twice
  in a row), then `Engine::start`s one from `params.config()` at the host's
  sample rate, and allocates `process`'s mono buffer at the host's maximum
  block size.
- **`deactivate`** drops the engine, which closes both threads' channels and
  lets them wind down. The ring's writer goes with it.
- **The editor** opens and closes independently of activation. While the
  plugin is inactive it shows the plugin's name instead of a spiral.

---

## 5. Module map

| Path | Responsibility |
|---|---|
| `lib.rs` | The plugin: `MelodyVisualizer`, `process`, activation, CLAP export. |
| `main.rs` | The standalone app: `nice_export_standalone`. |
| `params.rs` | `PluginParams`, their names and formatting, and `config()`. |
| `engine.rs` | `Engine`: starts the two threads, applies configs, renders frames. |
| `editor.rs` | The iced application, frame upload, the interval ring's labels. |
| `editor/pane.rs` | The control pane: stage rows, sliders, switches, the key row, the theme. |
| `editor/range_slider.rs` | The two-handle slider for the pitch range. |
| `audio/ring.rs` | Both ends of the capture ring, and the overrun count they share. |
| `async_processor.rs` | `AsyncProcessor<T>` — closure RPC to a background thread. |
| `note.rs` | Musical note and pitch-class math; the `note!` macro. |
| `app/config.rs` | `Config` plus the config-enum macros. |
| `spectrum/` | `Spectrum` types, spectrum thread, generators, transforms. |
| `graphic/` | `Graphic`/`GraphicBuffer`, graphic thread, generators. |
| `traits.rs` | `Configurable` — build or update a component from its config. |
| `error.rs` | Crate-wide `Error`. |
| `test_support/` | Shared fixtures: synthesized audio, a reader over fixed samples, and the wiring that builds a renderer from a `Config`. Public under the `testing` feature. |
| `xtask/` | nice-plug's bundler, as `cargo xtask bundle`. |

---

## 6. Invariants

Rules that keep the design intact. Breaking one needs a note in this document.

1. **`process` does not block, allocate, or lock.** It mixes, writes to the
   ring, sets a flag, and returns.
2. **Each pipeline stage has exactly one owner thread.** State crosses a
   thread boundary by moving through a channel, never by being shared.
3. **The parameters are the only source of truth.** A background thread's live
   objects are derived from them and are never read back as authority, and the
   editor sets them only through the host.
4. **Parameter IDs and the CLAP ID never change.** Hosts save them in every
   project that uses the plugin.
5. **The data path never queues.** Every stage drops or stalls instead of
   building a backlog.
6. **Data flows forward, control flows back.** The DSP never reaches into the
   editor; the visualizer never reaches into the DSP.

---

## 7. Target state and gaps

Where the code does not yet match the architecture above, or where a known
next step is planned. Each item is a candidate for its own change.

### Audio and the host

- **MIDI is not an input yet.** The planned MIDI visualization is a second
  `SpectrumGenerator` fed by the host's note events instead of audio, feeding
  the same transforms and spiral.
- **The DFT is complex.** Audio is real, so `realfft` would halve the
  generator's cost (JIM-296).
- **The standalone forgets its settings** between runs. The plugin does not:
  the host saves its parameters.

### DSP chain

- **The stages do not compose over their value ranges.** `DecibelConverter`
  emits up to 3.0 into a consumer that treats 1 as the peak, which is why it
  is commented out of `Config::default` and has no parameters. Either it
  normalizes to its own output range, or the `[0, ~1]` requirement moves onto
  the chain's output instead of living as the consumer's private assumption.
- **The transforms measure time in ticks, and the tick is adjustable.**
  `VolumeNormalizer::rate` is per spectrum, so raising the update rate
  shortens the time its running peak decays over without its own parameter
  moving. A rate in seconds, scaled by `generator.interval()`, would decouple
  them.
- `TransformChain::remove` and `reorder` exist, but nothing calls them: the
  chain is fixed.
- The spectrum thread's own loop — timing, backpressure, shutdown — has no
  tests. The chain it drives is covered without one, since `SpectrumRenderer`
  is drivable on its own.

### Visualizer and editor

- `SpiralGenerator` is the only generator, and `history_len()` is 1, so the
  history mechanism is never exercised. Either use it or simplify it away.
- **The frame's pixels are allocated per frame.** Recycle them if the
  allocation shows up in a profile.
- **The editor thread waits on the engine.** `apply` and `render` block until
  the threads answer. A frame is a millisecond or two, but a pixel-map rebuild
  after a resize or a spiral setting change takes tens of milliseconds, and the
  window does not respond meanwhile.
- **The spiral is drawn on the CPU and uploaded.** A shader drawing it on the
  GPU from the spectrum would remove the upload, the largest per-frame copy.
- A switched-off stage dims its row, but its sliders still respond: iced's
  sliders have no disabled state.
- Engine errors are logged, not shown in the window.
