#!/usr/bin/env bash
# Screenshots the standalone app's window, headless, with a sine tone playing
# into it.
#
# Starts a private dummy JACK server and jack_simple_client's 240 Hz tone,
# runs the debug standalone under Xvfb with its input on the tone, and saves
# the window to the given path after it has drawn for a while. Needs jackd2,
# xvfb and imagemagick.
#
#   scripts/screenshot.sh [out.png] [seconds]
set -euo pipefail

OUT="${1:-/tmp/melody-shot.png}"
SECONDS_TO_RUN="${2:-5}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo build --quiet

# A server of our own, so a JACK or PipeWire session on the machine is
# neither used nor disturbed.
export JACK_DEFAULT_SERVER="melody-shot-$$" JACK_NO_AUDIO_RESERVATION=1
LOG="$(mktemp -d)"
jackd -n "$JACK_DEFAULT_SERVER" -d dummy -r 48000 -p 512 >"$LOG/jackd.log" 2>&1 &
JACKD=$!
trap 'kill "$JACKD" "${TONE:-}" 2>/dev/null || true' EXIT
sleep 1
jack_simple_client tone >"$LOG/tone.log" 2>&1 &
TONE=$!
sleep 1

# The window opens at the editor's default size at the top-left of the screen.
xvfb-run -a -s '-screen 0 1280x800x24' bash -c '
	NICE_LOG="$1/app.log" target/debug/melody-visualizer \
		--backend jack --connect-jack-inputs tone:output1 &
	APP=$!
	sleep "$2"
	import -window root -crop 1000x700+0+0 +repage "$3"
	kill "$APP"
' _ "$LOG" "$SECONDS_TO_RUN" "$OUT"

echo "$OUT (app log: $LOG/app.log)"
