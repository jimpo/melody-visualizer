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
#
# XDOTOOL, when set, is a list of xdotool commands run against the window
# just before the screenshot, to click or drag the controls first:
#
#   XDOTOOL='mousemove 1000 26 click 1 sleep 1' scripts/screenshot.sh
set -euo pipefail

OUT="${1:-/tmp/melody-shot.png}"
SECONDS_TO_RUN="${2:-8}"
cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo build --quiet

# A server of our own, so a JACK or PipeWire session on the machine is
# neither used nor disturbed. The name is fixed because JACK's shared-memory
# registry has room for only a few servers, and a killed server keeps its
# entry until a server of the same name takes it over.
export JACK_DEFAULT_SERVER=melody-shot JACK_NO_AUDIO_RESERVATION=1
LOG="$(mktemp -d)"
jackd -n "$JACK_DEFAULT_SERVER" -d dummy -r 48000 -p 512 >"$LOG/jackd.log" 2>&1 &
JACKD=$!
# The tone goes first: a client still attached keeps the server's registry
# entry, so the next run could not start a server of the same name.
cleanup() {
	if [[ -n "${TONE:-}" ]]; then
		kill "$TONE" 2>/dev/null || true
		wait "$TONE" 2>/dev/null || true
	fi
	kill "$JACKD" 2>/dev/null || true
	wait "$JACKD" 2>/dev/null || true
}
trap cleanup EXIT
# A server takes a moment to accept clients, and a client started too early
# fails rather than waits, so start each piece once the last one is up.
wait_for_port() {
	for _ in $(seq 1 50); do
		jack_lsp 2>/dev/null | grep -qx "$1" && return
		sleep 0.1
	done
	echo "timed out waiting for JACK port $1 (see $LOG)" >&2
	exit 1
}
wait_for_port system:playback_1
jack_simple_client tone >"$LOG/tone.log" 2>&1 &
TONE=$!
wait_for_port tone:output1

# The window opens at the editor's default size at the top-left of the screen.
xvfb-run -a -s '-screen 0 1280x800x24' bash -c '
	NICE_LOG="$1/app.log" target/debug/melody-visualizer \
		--backend jack --connect-jack-inputs tone:output1 &
	APP=$!
	sleep "$2"
	if [[ -n "${XDOTOOL:-}" ]]; then
		# shellcheck disable=SC2086 # one word per xdotool argument
		xdotool $XDOTOOL
	fi
	import -window root -crop 1200x760+0+0 +repage "$3"
	kill "$APP"
' _ "$LOG" "$SECONDS_TO_RUN" "$OUT"

echo "$OUT (app log: $LOG/app.log)"
