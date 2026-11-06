# Development

Quick-start context for AI agents and developers working on Melody Visualizer.

For the design of the system — components, threads, channels — see
[ARCHITECTURE.md](ARCHITECTURE.md). [README.md](README.md) covers building
and installing the plugin.

## System dependencies

On Debian/Ubuntu:

```bash
sudo apt install build-essential pkg-config libasound2-dev
```

To screenshot the window headlessly (see
[below](#seeing-the-window-headlessly)) you also need:

```bash
sudo apt install jackd2 jack-example-tools xvfb imagemagick xdotool
```

## Build commands

```bash
cargo build                                   # Debug build: the plugin library and the standalone
cargo run --release -- --help                 # The standalone app's options
cargo xtask bundle melody-visualizer --release  # The CLAP bundle, in target/bundled/
```

`Cargo.toml` builds the dev profile at `opt-level = 1`. Unoptimised, the
spiral's pixel loop is an order of magnitude slower, which makes the debug
standalone stutter and the render gate measure the wrong thing.

## Formatting and linting

[prek](https://github.com/j178/prek) runs the checks in `prek.toml`:
`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`.

```bash
prek run --all-files   # Run both checks now
prek install           # Run them on every git commit
```

## Testing

```bash
cargo test
```

Unit tests live in `#[cfg(test)]` modules under `src/`; integration tests live
in `tests/` and reach the crate as a library (`use melody_visualizer::…`). The
`test_support` fixtures are public to `tests/` and `benches/` through the
`testing` feature, which the self dev-dependency in `Cargo.toml` enables for
those targets — no extra flag is needed on the command line.

Nothing in the suite needs a host, a display or an audio server. The editor
has unit tests only for its pure parts, such as the range slider's geometry;
the window itself is checked by eye, with the screenshot script below.

### Validating the plugin

[clap-validator](https://github.com/free-audio/clap-validator) loads the
bundle the way a host does and checks it against the CLAP spec: parameter
round trips, state save and load, activation, processing with odd block sizes.
Run it on a release bundle after changing the parameters or the `Plugin` impl:

```bash
cargo install --git https://github.com/free-audio/clap-validator clap-validator  # one-time setup
cargo xtask bundle melody-visualizer --release
clap-validator validate "target/bundled/Melody Visualizer.clap"
```

Every test passes today.

## Running automated checks

```bash
$ cargo fmt                                    # rustfmt.toml pins hard_tabs = true
$ cargo fmt --check                            # verify without rewriting
$ cargo clippy --all-targets -- -D warnings
```

Formatting and clippy are both clean today. Keep them that way: run both
before you hand work back. Suppress a lint only where fixing it would mean a
refactor beyond the change at hand, and always with the reason in a comment.

The codebase is indented with **hard tabs**. `rustfmt.toml` enforces this, so
run `cargo fmt` rather than matching it by hand.

## Logging

The plugin logs through nice-plug, which writes to stderr by default. Set
`NICE_LOG=/path/to/file` to log to a file instead, which is the only way to
see the log from inside a host. The engine logs through the `log` crate,
bridged into the same output. Once a second at debug level the spectrum thread
logs the loudest note it hears, which tells you audio is arriving without
looking at the window.

## Benchmarks

`benches/dsp.rs` measures each DSP stage on its own, saturated, in the unit
that stage converts: samples/s for the DFT, spectra/s for the log binning, the
generator as a whole, and each transform. `benches/graphic.rs` does the same
for the visualizer, in frames/s; see [The visualizer](#the-visualizer) below.

```bash
$ cargo bench                       # every benchmark, then the summary
$ cargo bench -- --test             # the summary alone, in a few seconds
$ cargo bench -- transform/diffuser # one group
```

The stages run in sequence on the one spectrum thread, so the chain's capacity
is the **reciprocal sum** of theirs, not the smallest of them. The summary
computes that, and reports headroom against the default update rate, once at
the default window and once at the longest the **Window** parameter reaches.
The window sets the generator's cost and nothing else's; the tick is the update
rate's alone.

### Recorded baseline

The absolute numbers here and in [The visualizer](#the-visualizer) are from
one machine, and vary by tens of percent between runs on it; a slower box
shifts them together. The ratios between rows are the part that should
reproduce.

Release build, 50 ms window (2400 samples) at 48 kHz, 1196 bins. The update
rate is 50 / sec, so the tick is 20 ms and **50 spectra/s** is what the chain
has to keep up with.

| Stage | Capacity | Headroom | % of tick |
|---|--:|--:|--:|
| Generator (DFT + binning) | 11,901 spectra/s | 238× | 0.42% |
| HarmonicSummation (8 harmonics) | 251,655 spectra/s | 5,033× | 0.02% |
| PowerMap (exponent 2) | 75,561 spectra/s | 1,511× | 0.07% |
| Diffuser (1/24 octave) | 239,262 spectra/s | 4,785× | 0.02% |
| VolumeNormalizer | 635,405 spectra/s | 12,708× | 0.01% |
| **Composed default chain** | **9,347 spectra/s** | **187×** | **0.53%** |

At the one-second window (48,000 samples) the generator's capacity falls to
about 1,250 spectra/s and the composed chain takes about **4%** of the same
tick. The generator is the whole of that cost: the transforms work on the grid,
which the window does not change.

The generator's cost grows with the samples in the window, so the tightest
corner is the longest window at the fastest update rate, 100 / sec, and the
highest sample rate the host runs at.

**There is no throughput problem at the defaults.** The default chain costs
about half a percent of its tick. The benchmarks exist to hold that, to catch
a regression, and to locate the cliff below — not to justify optimizing a path
with two orders of magnitude of headroom.

### The cliff

`Diffuser` is **O(bins²)**: convolution costs bins × window, and the window
length itself grows with bin density. At width 1/24 octave:

| Bins | Time | Capacity |
|---|--:|--:|
| 1196 | 4.22 µs | 236,700/s |
| 2392 | 15.56 µs | 64,300/s |
| 4784 | 58.01 µs | 17,200/s |

Every doubling of bins costs 4×. `HarmonicSummation` is linear in comparison:
bins × harmonics, about 3.7 µs at 1196 bins and 8 harmonics. Width is the same
story at a fixed bin count: 1/24 octave takes 4.2 µs, one octave 97 µs, ten
octaves 830 µs — the last of those is 4% of the tick, from a parameter the
editor already offers.

Bin count is `samples_per_octave`, a config value no parameter moves: 180 by
default, which gives 720 bins over the default D2 to D6 range.

`tests/dsp_budget.rs` is the gate the ordinary test run applies: at the
fastest update rate, the composed chain must stay inside half a tick at the
shortest window, the default and the longest the **Window** parameter reaches.
Benchmarks catch nothing if nobody runs them; that test does.

### The visualizer

`benches/graphic.rs` measures the graphic stage in frames/s against the
editor's fastest frame rate, 64 fps. It needs no display: the spiral writes its
pixels in plain Rust.

```bash
$ cargo bench --bench graphic            # the sweeps, then the summary
$ cargo bench --bench graphic -- --test  # the summary alone
```

**This is the stage with the least headroom.** The whole DSP chain costs half
a percent of its tick; one frame at 1600x1000 costs about a twentieth of a
frame. This is where optimisation effort belongs if it is ever needed.

Cost scales **opposite to the DSP**: it is driven by pixel area, not bin count.

| Surface | % of frame | Max fps |  | Bins @ 1600x1000 | % of frame |
|---|--:|--:|---|---|--:|
| 640x480 | 1.06% | 6,049 |  | 180 (45/octave) | 4.78% |
| 1280x800 | 3.15% | 2,031 |  | 720 (default) | 4.84% |
| 1600x1000 | 4.81% | 1,331 |  | 2880 (720/octave) | 5.08% |
| 3840x2160 | 22.6% | 283 |  | | |

`SpiralGenerator` keeps a map with an entry per pixel: the two neighbouring
bins it lies between, how far toward the upper one, and how brightly it is
lit. A frame is one pass over that map, two lookups and a blend a pixel, so its
cost follows the pixel count and the bin count does not enter it. The blend
makes the ribbon a smooth gradient along its length, whatever the bin count.
So **adding spectral resolution is free for the visualizer, while resizing the
window is what costs** — anyone tuning `samples_per_octave` needs that
alongside the DSP's quadratic in bins.

The brightness in the map is a glow in linear light: a flat core of full
brightness round the ribbon's centre line, then a Gaussian from the core's
edge. The spiral config sets the core's half-width and the Gaussian's sigma,
both in octaves, and the parameters show them in cents. The glow is stored
already encoded to sRGB with a power-law gamma. Encoding distributes over
multiplication for a power law, so the multiply of two encoded bytes stays
correct in light and the pixel loop needs no conversion.

Building the map costs far more than drawing from it. It inverts the spiral at
every pixel, which takes about 30 ms at 1280x800, 60 ms at 1920x1080 and
230 ms at 3840x2160. A change of size, grid or spiral setting marks the map
stale and the next frame rebuilds it, so a window resize or a slider drag
costs one rebuild a frame. Frames drop while it lasts, and spectrum ticks with
them: the spectrum thread gets its buffer back only between the graphic
thread's commands, so it skips, and logs, every tick a rebuild outlasts.

The benchmark covers the draw only. The frame then goes to iced and up to the
GPU, about 2.4 MB at the default window size. The editor skips drawing while
the last frame is still uploading rather than queueing frames.

`tests/render_budget.rs` is the gate, at 1280x800. One threshold covers both
build profiles because `Cargo.toml` optimises the dev profile.

## Seeing the window headlessly

The window can be run and checked with **no physical display and no audio
hardware**. `scripts/screenshot.sh` starts a private `jackd -d dummy` server
and `jack_simple_client`'s sine tone (240 Hz, which the spectrum thread logs as
B♭3), runs the debug standalone under Xvfb with its input on the tone, waits,
and screenshots the window:

```bash
$ scripts/screenshot.sh [out.png] [seconds]   # default /tmp/melody-shot.png, 8 s
```

It prints the screenshot's path and the app log's. The JACK server has a fixed
name, `melody-shot`, so a server or PipeWire session you are running yourself
is neither used nor disturbed. The window opens at the editor's default size,
1200x760, at the top-left of the screen, so a pixel in the screenshot is a
pixel in the window.

To check a control, set `XDOTOOL` to xdotool commands that run against the
window just before the shot. The pane starts at x = 780; its first row's
centre is at y = 26, and each row is 53 px below the last:

```bash
# Open the Spiral stage, turn the interval ring on, and move the pointer away.
$ XDOTOOL='mousemove 900 290 click 1 sleep 1 mousemove 1160 540 click 1 sleep 1 mousemove 400 750' \
	scripts/screenshot.sh
```

A spiral with one bright band proves the whole path: audio in, spectrum,
frame, upload. The empty window with "Melody Visualizer" in the middle means
the engine is not running — the standalone could not start or the JACK server
went away; the app log says which.

JACK allows only a handful of servers at once, sharing one registry in
`/dev/shm`. The script stops its tone client before its server so the next run
can reuse the name. If a run is killed hard and the next fails with "Too many
servers already active", look for stale `jack-*` entries in `/dev/shm` left by
`melody-shot`.

### Putting a screenshot in a pull request

`scripts/post-screenshot.sh` publishes an image and prints the markdown line
that shows it. Paste that line into the pull request description.

```bash
$ scripts/screenshot.sh                            # -> /tmp/melody-shot.png
$ scripts/post-screenshot.sh /tmp/melody-shot.png
![melody-shot](https://raw.githubusercontent.com/jimpo/melody-visualizer/<sha>/melody-shot.png)
```

The image is committed to an **orphan branch** `screenshots/<the current
branch>` through the GitHub API — no parent, so no history reaches it, and
nothing local is touched. The printed URL names the commit rather than the
branch, so a later screenshot on the same branch cannot move it. Set the
bookmark before posting: its name is what pairs the images with the pull
request, and `SLUG` overrides it.

Merging a pull request deletes its head branch, which leaves
`screenshots/<that branch>` orphaned, and the next run of the script deletes
every screenshot branch in that state before publishing. To drop a set now:

```bash
$ gh api -X DELETE repos/jimpo/melody-visualizer/git/refs/heads/screenshots/<branch>
```

## Style guide

`rustfmt` and Clippy enforce the mechanical formatting rules; see [Running
automated checks](#running-automated-checks). The conventions below are the
ones tooling cannot check. They are **rules**: a change that breaks one needs a
stated reason. The judgment calls — where two good options trade off against
each other — are in [Principles](#principles).

### Code comments

**Comments explain current behavior, not change history.** Do not write
comments that reference how the code used to work ("previously X", "changed
from A to B", "no longer calls Y"). A comment must make sense against the
current code alone, with no knowledge of what came before. What changed and
why belongs in the commit message.

### Documentation

Write documentation and commit messages in the **present tense**.

```
❌ This function will return the right answer
✅ This function returns the right answer

❌ Fixed the overrun counter
✅ Fix the overrun counter
```

Follow the [rustdoc book](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)
on structure: a short sentence explaining what the item is, then a more
detailed explanation, then examples where they help.

Add code examples to the parts of the crate that are reusable on their own —
`async_processor`, `traits`, and the `Spectrum` types — and to any non-obvious
behavior or edge case. Doc examples run under `cargo test --doc`, so they
double as regression tests. The plugin, editor and engine wiring need prose,
not examples: nothing calls them but nice-plug.

### Naming

Prefer **longer, descriptive names**, including for generic type parameters.
Use CamelCase identifiers for type parameters rather than single letters,
especially when a function or type has more than one.

**Use namespacing.** An identifier that is unambiguous inside its module does
not repeat the module's name. Every transform module holds a plain `Config` —
`diffuser::Config`, not `DiffuserConfig` — because the module already says
which one it is. A caller that needs the longer form renames on import:
`use diffuser::Config as DiffuserConfig`.

### Functional style

Prefer iterator combinators (`map`, `filter`, `fold`, `sum`) over loops that
drive mutable state, and `iter::zip(a, b)` over `a.iter().zip(&b)`. An
algorithm with substantial mutable state — the DSP transforms that update a
running envelope in place, for example — is the exception, and is clearer
written imperatively.

```rust
// Good
let total = data.iter().sum::<f64>();
let scaled = iter::zip(data, weights)
	.map(|(value, weight)| value * weight)
	.collect::<Vec<_>>();

// Poor
let mut total = 0.0;
for value in data.iter() {
	total += value;
}
```

### Error handling

**Do not call `unwrap` outside of test code.** Return or propagate an `Err`,
or call `expect` with an explanation of why the call cannot panic. `unwrap` is
fine in `#[cfg(test)]` modules and in `tests/`.

**Internal code documents preconditions and asserts them; it does not return
`Result`.** Assertions keep the internal interfaces small and make the contract
visible at the definition.

```rust
/// Scales every amplitude value in place by the weight of its frequency bin.
///
/// # Preconditions
/// - `data.len()` must equal `params.samples()`
fn apply_weights(data: &mut [f64], params: &SpectrumParams) {
	assert_eq!(data.len(), params.samples());
	// ...
}
```

**Return an error where input the crate does not control could otherwise
cause a panic.** Those boundaries are:

- **Config.** A `Config` may name a transform that does not exist or an entry
  of the wrong shape, so `app::config` returns `Error::MissingTransform` and
  `Error::UnexpectedConfigEntry`.
- **Cross-thread channels.** A peer thread may be gone by the time a message
  is sent, which is `Error::Communication`.
- **Thread creation**, which the OS can refuse: `Error::ThreadSpawnFailure`.

Inside those boundaries — a transform applied to a spectrum the pipeline
built, a renderer handed a buffer the engine allocated — use preconditions.
`process` is stricter still: it may not block, allocate, or lock, so it may
not panic either (ARCHITECTURE.md, invariant 1). An error the editor cannot
act on is logged, once, not once a frame.

### Turbofish over type annotations

Resolve type ambiguity with a turbofish rather than an annotation on the
local, so the type sits at the call site that needs it.

```rust
// Good
let names = ports.iter().map(|port| port.name()).collect::<Vec<_>>();

// Bad
let names: Vec<_> = ports.iter().map(|port| port.name()).collect();
```

### Visibility

Control visibility at the **ancestor module**, not per item. An item that
should not escape the crate can be plain `pub` inside a module that is not
itself `pub` — the private ancestor already blocks external reach, and no item
needs an annotation. Reach for `pub(crate)` or `pub(super)` only where that
cannot express the intent:

- a `pub mod` that exposes an API but holds helpers other modules must call;
- a field of a `pub` struct that the crate must read and callers must not,
  since fields have no module-level escape hatch.

### Generic functions over trait methods

Write a generic function unless the logic must vary by implementor. Add a
trait method with a default implementation only when at least one implementor
overrides it.

### Dependencies

Before adding a crate, check that it is widely used (downloads and recent
releases on `crates.io`), still maintained, and backed by an organization
rather than a single person. Prefer what nice-plug already depends on.

### Tests

New functionality comes with tests for the expected behavior and the edge
cases. Two conventions keep the suite reproducible:

- **Seed randomness deterministically.** Draw from `StdRng::seed_from_u64(0)`
  rather than from entropy, so a failure reproduces.
- **Reuse the shared fixtures.** `src/test_support/` reaches `tests/` and
  `benches/` through the `testing` feature. Extend it rather than rebuilding
  common setup per test.

## Principles

The style rules above are pass/fail. The principles here are **tradeoff
guidelines**. Each reads *Prefer X over Y*, and the **Why** and **Reconsider
when** are the substance — a change that leans the other way is a discussion,
not a defect.

### Simplicity

Simplicity is the foundational design goal. It is how easy the code is to
understand and how fast that understanding transfers to someone else.

Simple code is **loosely coupled**: to understand a module you need the public
interfaces of its dependencies and almost nothing about their implementations.
The component boundaries in [ARCHITECTURE.md](ARCHITECTURE.md) exist to hold
that property. The principles below are all consequences of it.

### No backwards compatibility

- **Prefer** *strongly* clean, targeted interfaces **over** interfaces kept
  compatible with their old shape.
- **Why:** nothing outside this repo consumes the crate. A deprecated path
  kept alive costs every future reader. Isolate a breaking interface change
  into its own commit so the mechanical part is legible on its own.
- **The exception** is what hosts save: the CLAP ID and the parameter IDs.
  Changing one breaks every saved project that uses the plugin.
- **Reconsider when:** the break turns into a whole-codebase edit that buys
  little.

### Avoid premature generalization

- **Prefer** *weakly* simple, locally readable logic **over** perfectly DRY
  code.
- **Why:** readability matters more here than extensibility. Structure
  repeated in a few places with small variations beats one combined,
  parameterized, confusing instance.
- **Reconsider when:** the generalization is just as simple as the copies, and
  decouples the logic in a way that makes both sides easier to understand.

### Don't trade loose coupling for method-call convenience

- **Prefer** keeping a function where the module boundary separates
  responsibilities **over** making it a method on a type it happens to take as
  an argument.
- **Why:** converting a free function to a method is a win only when the type
  genuinely owns that responsibility. When several arguments are equally
  central, or when the move makes a plain data module import the machinery it
  had no reason to know about, the `x.method()` syntax is bought with real
  coupling. This is what keeps the transforms out of `Spectrum`.
- **Reconsider when:** the type really is the one thing the function is about,
  and moving it adds no import across the data/algorithm boundary.

### Cohesive commits

- **Prefer** several small, logically cohesive commits **over** one large
  monolithic change.
- **Why:** a focused commit is easier to review, to reason about in isolation,
  and to revert. A sequence of well-scoped commits communicates the design
  incrementally: each step stands on its own.
- **Reconsider when:** the parts are genuinely inseparable — a mechanical
  rename where any split leaves an intermediate commit that does not compile.

## Key terminology

| Term | Definition |
|---|---|
| **Engine** | The spectrum and graphic threads, started from a `Config` for one sample rate. Runs while the plugin is active |
| **Overrun** | Audio `process` dropped because the capture ring was full. Counted in an atomic on `SampleWriter`, read on the spectrum thread and logged as a warning |
| **Spectrum** | One frame of frequency-domain data: non-negative amplitude values plus the `SpectrumParams` that give each bin its frequency |
| **SpectrumParams** | The log-spaced frequency grid. Shared as an `Arc` and compared with `Arc::ptr_eq`; a pointer mismatch is what invalidates downstream caches |
| **SpectrumBuffer** | A `Spectrum` with no meaningful contents — the recycled allocation that cycles back from the graphic thread |
| **Spectrum transform** | One stage of the DSP chain (`HarmonicSummation`, `PowerMap`, `Diffuser`, `VolumeNormalizer`, `DecibelConverter`), applied in a configured order |
| **Generator** | The head of a pipeline. A `SpectrumGenerator` makes a spectrum from samples; a `GraphicGenerator` makes a graphic from spectra |
| **Spectrum tick** | The spectrum thread's timer, at `generator.interval()` — one over the update rate. It is the pipeline's only timed producer |
| **Frame poll** | nice-plug-iced's `poll_events`, up to 64 times a second after `process` notifies. Each one draws a frame |
| **Stage** | A row of the editor's control pane: the spectrum, a transform, or the spiral |
| **`AsyncProcessor<T>`** | The RPC handle. Ships a `FnOnce(&mut T)` to the thread that owns `T` and returns a future of its result |
