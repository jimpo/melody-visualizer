#!/usr/bin/env bash
#
# Play an audio file into the visualizer, and close the window when it ends.
#
# The app is launched under pw-jack and left to register its input port. The
# file is then played with pw-cat, which WirePlumber routes to the system
# output as it would any player, and pw-link feeds the same stream to the app's
# input as well — the same shape as connect-test-tone.sh, with jack_connect.
# The track is heard and analysed at once, and the connection reaches the app
# from outside, which the control pane already reports like any other.
# Whichever of the two ends first takes the other with it: pw-cat returns at
# the end of the file, and closing the window ends the playback.
#
# Prerequisites: PipeWire, with pw-jack, pw-cat and pw-link (Debian/Ubuntu:
# pipewire-jack and pipewire-bin).
#
# Usage:
#   scripts/play-file.sh <audio-file>
#   CHANNEL=FR scripts/play-file.sh track.flac    # the right channel instead of the left
#   APP_BIN="cargo run" scripts/play-file.sh track.flac   # a debug build
#
set -euo pipefail

# Resolved before the cd below, and checked to exist before anything is built.
FILE="$(realpath -e -- "${1:?usage: $0 <audio-file>}")"
APP_BIN="${APP_BIN:-cargo run --release}"  # how to start the app, from the repo root
INPUT="${INPUT:-Melody Visualizer:input}"  # the app's input port
CHANNEL="${CHANNEL:-FL}"                    # which of the file's channels to feed it: FL or FR
PLAYER_NODE="play-file"                     # the name the stream takes in the graph

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

log() { printf '>>> %s\n' "$*" >&2; }

# A second instance of the app would hand off to the running one and exit, so
# there would be no window of ours to close.
if pw-link -i 2>/dev/null | grep -qxF "$INPUT"; then
	log "ERROR: '${INPUT}' already exists — is the app already running?"
	exit 1
fi

# setsid puts the app in its own process group, so the whole chain — cargo and
# the binary it runs — can be stopped by group id.
log "launching app: pw-jack ${APP_BIN}"
# shellcheck disable=SC2086 # APP_BIN is a command line, so it splits on purpose
setsid pw-jack $APP_BIN &
APP_PGID=$!
PLAYER_PID=""

cleanup() {
	log "stopping ..."
	kill -TERM -"${APP_PGID}" 2>/dev/null || true
	[[ -n "$PLAYER_PID" ]] && kill -TERM "$PLAYER_PID" 2>/dev/null || true
	# Let both go before returning, so a run started right after this one does
	# not find the old port still registered.
	wait 2>/dev/null || true
}
trap cleanup EXIT

# Wait until `port` is listed by `pw-link <flag>`, for as long as `pid` lives.
# A debug build can take a while, so there is no fixed timeout.
wait_for_port() {
	local flag="$1" port="$2" pid="$3"
	until pw-link "$flag" 2>/dev/null | grep -qxF "$port"; do
		kill -0 "$pid" 2>/dev/null || { log "ERROR: exited before registering ${port}"; exit 1; }
		sleep 0.2
	done
}

log "waiting for ${INPUT} ..."
wait_for_port -i "$INPUT" "$APP_PGID"

# The app is linked below rather than named as --target: WirePlumber cannot
# resolve a JACK client by name, and the default sink is wanted anyway.
log "playing ${FILE} ..."
pw-cat --playback -P "{ node.name = \"${PLAYER_NODE}\" }" "$FILE" &
PLAYER_PID=$!

# The app has one mono input, so it is fed the channel asked for — or the only
# one a mono file has.
until PLAYER_PORT="$(pw-link -o 2>/dev/null | grep -m1 -E "^${PLAYER_NODE}:output_(${CHANNEL}|MONO)$")"; do
	kill -0 "$PLAYER_PID" 2>/dev/null || { log "ERROR: the player exited before registering a port"; exit 1; }
	sleep 0.2
done
log "linking ${PLAYER_PORT} -> ${INPUT}"
pw-link "$PLAYER_PORT" "$INPUT"

# Whichever ends first — the file, or the window — ends the run.
wait -n || true
log "done"
