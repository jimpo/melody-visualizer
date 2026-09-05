# Development

Quick-start context for AI agents and developers working on Melody Visualizer.

For the design of the system — components, threads, channels — see
[ARCHITECTURE.md](ARCHITECTURE.md).

## System dependencies

The crate links against GTK 4 and JACK. On Debian/Ubuntu:

```bash
sudo apt install build-essential clang libgtk-4-dev libjack-jackd2-dev
```

To run the test suite you also need `jackd` itself: the audio integration tests
start a server of their own. To run the app headlessly (see
[below](#running-the-gui-headlessly)) you need the rest of these too:

```bash
sudo apt install jackd2 xvfb dbus-x11 imagemagick
```

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
```

> **`cargo test` currently aborts, even though every test passes.** The
> `pubsub::tests::*` tests drive a glib main loop through
> `test_support::run_in_glib_main_loop`, and that loop runs on the process-global
> default `MainContext`. libtest runs many tests in one process, so the second
> such test either touches a value glib pinned to another thread or polls a task
> the first test left behind. Either way glib panics inside a non-unwinding
> context and the process takes a `SIGABRT`. nextest's process-per-test isolation
> sidesteps it entirely.
>
> The real fix is for `run_in_glib_main_loop` to build a fresh `MainContext`
> instead of using the thread default, and to drop its leftover yield task when
> the test ends. Until then, use nextest.

### The tests that need a JACK server

`tests/audio_source.rs` exercises `AudioSource` against a real server, and it
runs as part of the ordinary suite — there is nothing to start first and nothing
to opt into. Each test spawns a private `jackd -d dummy` through
`test_support::jackd::Server`, so `jackd` must be installed (`apt install
jackd2`) and a server you are running yourself is neither used nor disturbed.

Which server a JACK client opens comes from the `JACK_DEFAULT_SERVER`
environment variable — `ClientOptions::SERVER_NAME` cannot be used, because
`Client::new` omits the varargs that flag reads the name from. A process has one
environment, so **these tests need nextest's process-per-test isolation**;
`Server::start` panics with that advice rather than silently talking to the
wrong server.

JACK also allows only a handful of servers at once, all sharing one
shared-memory registry. `.config/nextest.toml` caps how many of these tests run
side by side, which is what keeps a many-core machine from exhausting it.

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

## Benchmarks

`benches/dsp.rs` measures each DSP stage on its own, saturated, in the unit that
stage converts: samples/s for the DFT, spectra/s for the log binning, the
generator as a whole, and each transform. `benches/graphic.rs` does the same for
the visualizer, in frames/s; see [The visualizer](#the-visualizer) below.

```bash
$ cargo bench                       # every benchmark, then the summary
$ cargo bench -- --test             # the summary alone, in a few seconds
$ cargo bench -- transform/diffuser # one group
```

The stages run in sequence on the one spectrum thread, so the chain's capacity
is the **reciprocal sum** of theirs, not the smallest of them. The summary
computes that, and reports headroom against the tick rate the configured window
implies rather than a fixed constant — doubling the DFT window halves the rate
demanded of every transform while leaving its cost untouched.

### Recorded baseline

The absolute numbers here and in [The visualizer](#the-visualizer) are from one
machine; a slower box shifts them together. The ratios between rows are the part
that should reproduce.

Release build, 2048-sample window at 48 kHz, 1196 bins. The tick is 21.333 ms,
so **46.9 spectra/s** is what the chain has to keep up with.

| Stage | Capacity | Headroom | % of tick |
|---|--:|--:|--:|
| Generator (DFT + binning) | 48,232 spectra/s | 1,029× | 0.10% |
| Diffuser (1/24 octave) | 239,295 spectra/s | 5,105× | 0.02% |
| VolumeNormalizer | 518,970 spectra/s | 11,071× | 0.01% |
| DecibelConverter | 165,322 spectra/s | 3,527× | 0.03% |
| **Composed default chain** | **37,644 spectra/s** | **803×** | **0.12%** |

**There is no throughput problem.** The chain costs a tenth of a percent of its
budget. The benchmarks exist to hold that, to catch a regression, and to locate
the cliff below — not to justify optimizing a path with three orders of
magnitude of headroom.

### The cliff

`Diffuser` is **O(bins²)**: convolution costs bins × window, and the window
length itself grows with bin density. At width 1/24 octave:

| Bins | Time | Capacity |
|---|--:|--:|
| 1196 (default) | 4.22 µs | 236,700/s |
| 2392 | 15.56 µs | 64,300/s |
| 4784 | 58.01 µs | 17,200/s |

Every doubling of bins costs 4×. Width is the same story at a fixed bin count:
1/24 octave takes 4.2 µs, one octave 97 µs, ten octaves 830 µs — the last of
those is 4% of the tick, from a slider the GUI already offers.

Bin count is `samples_per_octave`, an internal `Config` field with no GUI
control, and exposing it is deferred. The benchmarks make the cliff visible so
that decision can be revisited on evidence.

`tests/dsp_budget.rs` is the gate the ordinary test run applies: the composed
chain must stay well inside one tick. Benchmarks catch nothing if nobody runs
them; that test does.

### The visualizer

`benches/graphic.rs` measures the graphic stage in frames/s against the 25 fps
frame timer. It needs no display: cairo's `ImageSurface` is CPU rasterisation,
so thousands of frames render with no X server.

```bash
$ cargo bench --bench graphic            # the sweeps, then the summary
$ cargo bench --bench graphic -- --test  # the summary alone
```

**This is the stage with the least headroom.** The whole DSP chain costs a tenth
of a percent of its tick; one frame at 1600x1000 costs about a fifth of the 40 ms
frame. Six times' headroom against the DSP's five hundred, so this is where
optimisation effort belongs if it is ever needed.

Cost scales **opposite to the DSP**: it is driven by pixel area, not bin count.

| Surface | % of frame | Max fps |  | Bins @ 1600x1000 | % of frame |
|---|--:|--:|---|---|--:|
| 640x480 | 6.2% | 404 |  | 299 (45/octave) | 19.1% |
| 1280x800 | 11.3% | 221 |  | 1196 (default) | 21.0% |
| 1600x1000 | 21.9% | 114 |  | 4784 (720/octave) | 30.2% |
| 3840x2160 | 69.3% | 36 |  | | |

Sixteen times the bins costs 1.6 times the time; 6.7 times the pixels costs 3.5
times. cairo rasterising the mesh gradient over the surface is what the frame
goes on, not the 2,392 mesh patches. So **adding spectral resolution is nearly
free for the visualizer, while resizing the window is what costs** — anyone
tuning `samples_per_octave` needs that alongside the DSP's quadratic in bins.

The `render/background` group fills the surface and paints no mesh.
`render/spiral` minus `render/background` is the mesh paint, which is the number
to look at before clipping that paint to the annulus.

`tests/render_budget.rs` is the gate, at 1280x800. One threshold covers both
build profiles: nearly all of the time is inside cairo, which is compiled
optimized either way, so an unoptimized build measures within a fifth of a
release one.

## Style guide

`rustfmt` and Clippy enforce the mechanical formatting rules; see [Running
automated checks](#running-automated-checks). The conventions below are the ones
tooling cannot check. They are **rules**: a change that breaks one needs a
stated reason. The judgment calls — where two good options trade off against
each other — are in [Principles](#principles).

### Code comments

**Comments explain current behavior, not change history.** Do not write comments
that reference how the code used to work ("previously X", "changed from A to B",
"no longer calls Y"). A comment must make sense against the current code alone,
with no knowledge of what came before. What changed and why belongs in the
commit message.

### Documentation

Write documentation and commit messages in the **present tense**.

```
❌ This function will return the right answer
✅ This function returns the right answer

❌ Fixed the overrun counter
✅ Fix the overrun counter
```

Follow the [rustdoc book](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)
on structure. Each item's docs read:

```
[short sentence explaining what it is]

[more detailed explanation]

[at least one code example that users can copy/paste to try it]

[even more advanced explanations if necessary]
```

`src/lib.rs` carries crate-level `//!` documentation: a one-sentence summary of
what the crate does, the main types with a one-line description each, and a
pointer to [ARCHITECTURE.md](ARCHITECTURE.md) for the component and thread model.

Add code examples to the parts of the crate that are reusable on their own —
`pubsub`, `async_processor`, `traits`, and the `Spectrum` types — and to any
non-obvious behavior or edge case. Doc examples run under `cargo test --doc`, so
they double as regression tests. The GUI, controller, and app-wiring modules
need prose, not examples: nothing calls them but the binary.

### Naming

Prefer **longer, descriptive names**, including for generic type parameters. Use
CamelCase identifiers for type parameters rather than single letters, especially
when a function or type has more than one.

**Use namespacing.** An identifier that is unambiguous inside its module does not
repeat the module's name. Every transform module holds a plain `Config` —
`diffuser::Config`, not `DiffuserConfig` — because the module already says which
one it is. A caller that needs the longer form renames on import:
`use diffuser::Config as DiffuserConfig`.

### Functional style

Prefer iterator combinators (`map`, `filter`, `fold`, `sum`) over loops that
drive mutable state, and `iter::zip(a, b)` over `a.iter().zip(&b)`. An algorithm
with substantial mutable state — the DSP transforms that update a running
envelope in place, for example — is the exception, and is clearer written
imperatively.

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

**Do not call `unwrap` outside of test code.** Return or propagate an `Err`, or
call `expect` with an explanation of why the call cannot panic. `unwrap` is fine
in `#[cfg(test)]` modules and in `tests/`.

```rust
// `chunk` comes from `chunks_exact(SAMPLE_SIZE)`, so the conversion is total.
let sample = f32::from_ne_bytes(
	chunk
		.try_into()
		.expect("chunks_exact yields chunks of SAMPLE_SIZE bytes"),
);
```

> The GUI modules still hold a couple of dozen `unwrap` calls that predate this
> rule, nearly all of them `gtk::Builder::object` lookups in `gui/window.rs` and
> `gui/control_pane.rs`. They are a cleanup backlog, not a precedent. New code
> follows the rule; code you touch for another reason is a good place to fix
> one.

**Internal code documents preconditions and asserts them; it does not return
`Result`.** Assertions keep the internal interfaces small and make the contract
visible at the definition.

```rust
/// Scales every power value in place by the weight of its frequency bin.
///
/// # Preconditions
/// - `data.len()` must equal `params.samples()`
fn apply_weights(data: &mut [f64], params: &SpectrumParams) {
	assert_eq!(data.len(), params.samples());
	// ...
}
```

**Return an error where input the crate does not control could otherwise cause a
panic.** Those boundaries are:

- **JACK.** The server can refuse a client, drop a port, or fail to activate at
  any moment. `AudioSource::new` returns `Error::Jack` / `Error::JackStatus`;
  it does not assert.
- **Config.** A `Config` may name a transform that does not exist or an entry of
  the wrong shape, so `app::config` returns `Error::MissingTransform` and
  `Error::UnexpectedConfigEntry`.
- **Cross-thread channels.** A peer thread may be gone by the time a message is
  sent, which is `Error::Communication` and `Error::PubSub`.
- **Allocation with a size the user chose**, such as the capture ring buffer:
  `Error::RingBufferAllocFailure`.

Inside those boundaries — a transform applied to a spectrum the pipeline built,
a renderer handed a surface the GUI owns — use preconditions. The JACK real-time
callback is stricter still: it may not block, allocate, or lock, so it may not
panic either (ARCHITECTURE.md, invariant 1).

### Turbofish over type annotations

Resolve type ambiguity with a turbofish rather than an annotation on the local,
so the type sits at the call site that needs it.

```rust
// Good
let names = ports.iter().map(|port| port.name()).collect::<Vec<_>>();

// Bad
let names: Vec<_> = ports.iter().map(|port| port.name()).collect();
```

### Visibility

Control visibility at the **ancestor module**, not per item. An item that should
not escape the crate can be plain `pub` inside a module that is not itself
`pub` — the private ancestor already blocks external reach, and no item needs an
annotation. Reach for `pub(crate)` or `pub(super)` only where that cannot express
the intent:

- a `pub mod` that exposes an API but holds helpers other modules must call;
- a field of a `pub` struct that the crate must read and callers must not, since
  fields have no module-level escape hatch.

### Generic functions over trait methods

Write a generic function unless the logic must vary by implementor. Add a trait
method with a default implementation only when at least one implementor
overrides it.

### Dependencies

Before adding a crate, check that it is widely used (downloads and recent
releases on `crates.io`), still maintained, and backed by an organization rather
than a single person.

### Tests

New functionality comes with tests for the expected behavior and the edge cases.
Two conventions keep the suite reproducible:

- **Seed randomness deterministically.** Draw from `StdRng::seed_from_u64(0)`
  rather than from entropy, so a failure reproduces.
- **Reuse the shared fixtures.** `src/test_support.rs` reaches `tests/` and
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
- **Why:** nothing outside this repo consumes the crate. A deprecated path kept
  alive costs every future reader. Isolate a breaking interface change into its
  own commit so the mechanical part is legible on its own.
- **Reconsider when:** the break turns into a whole-codebase edit that buys
  little.

### Avoid premature generalization

- **Prefer** *weakly* simple, locally readable logic **over** perfectly DRY code.
- **Why:** readability matters more here than extensibility. Structure repeated
  in a few places with small variations beats one combined, parameterized,
  confusing instance.
- **Reconsider when:** the generalization is just as simple as the copies, and
  decouples the logic in a way that makes both sides easier to understand.

### Don't trade loose coupling for method-call convenience

- **Prefer** keeping a function where the module boundary separates
  responsibilities **over** making it a method on a type it happens to take as an
  argument.
- **Why:** converting a free function to a method is a win only when the type
  genuinely owns that responsibility. When several arguments are equally central,
  or when the move makes a plain data module import the machinery it had no
  reason to know about, the `x.method()` syntax is bought with real coupling.
  This is what keeps the transforms out of `Spectrum` and the views free of
  controller state.
- **Reconsider when:** the type really is the one thing the function is about,
  and moving it adds no import across the data/algorithm boundary.

### Cohesive commits

- **Prefer** several small, logically cohesive commits **over** one large
  monolithic change.
- **Why:** a focused commit is easier to review, to reason about in isolation,
  and to revert. A sequence of well-scoped commits communicates the design
  incrementally: each step stands on its own.
- **Reconsider when:** the parts are genuinely inseparable — a mechanical rename
  where any split leaves an intermediate commit that does not compile.

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

### Putting a screenshot in a pull request

`scripts/post-screenshot.sh` publishes an image and prints the markdown line that
shows it. Paste that line into the pull request description.

```bash
$ scripts/run-headless.sh                          # -> /tmp/melody-shot.png
$ scripts/post-screenshot.sh /tmp/melody-shot.png
![melody-shot](https://raw.githubusercontent.com/jimpo/melody-visualizer/<sha>/melody-shot.png)
```

The image is committed to an **orphan branch** `screenshots/<the current
branch>` through the GitHub API — no parent, so no history reaches it, and
nothing local is touched. The printed URL names the commit rather than the
branch, so a later screenshot on the same branch cannot move it. Set the
bookmark before posting: its name is what pairs the images with the pull
request, and `SLUG` overrides it.

**Cleanup needs nobody.** The branch pairing is also what expires the images:
merging a pull request deletes its head branch, which leaves
`screenshots/<that branch>` orphaned, and the next run of the script deletes
every screenshot branch in that state before publishing. Posting a screenshot is
what collects the last one's. To drop a set now rather than then:

```bash
$ gh api -X DELETE repos/jimpo/melody-visualizer/git/refs/heads/screenshots/<branch>
```

The pairing reads `screenshots/<branch>` rather than `<branch>/screenshots`
because git forbids a ref that is a directory prefix of another:
`refs/heads/X` and `refs/heads/X/screenshots` cannot both exist.

Two routes that look easier are worse. A **release asset** cannot be rendered at
all: it redirects to a signed URL that expires within the hour and answers
`application/octet-stream`, which GitHub's image proxy refuses. **Git LFS** moves
the bytes out of the packfile but not out of the project — the objects and the
storage they bill for are kept forever, and every checkout materialises them.
Deleting one branch is what ends a screenshot's life.

## Known upstream workarounds

One workaround is load-bearing. It should be re-checked against current upstream
before being copied or extended.

| Where | Why |
|---|---|
| `audio/ports.rs` — `RetiredPorts` hides an unregistered port until JACK stops listing it | [jack2#617](https://github.com/jackaudio/jack2/issues/617). The port list is not immediately consistent after the notification. `a_port_leaves_the_input_list_the_moment_it_is_unregistered` is the test that fails if upstream fixes it and the workaround is dropped. |

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
