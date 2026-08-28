#!/usr/bin/env bash
#
# Run the melody-visualizer GUI headlessly and capture a screenshot.
#
# The app is a GTK 4 program that *requires* both a display and a running JACK
# server (it opens its client with NO_START_SERVER, so AppController::new fails
# without one). This script brings up the two missing pieces in a headless
# environment, launches the app, screenshots the virtual framebuffer, and (by
# default) stops the app again.
#
#   1. a dummy-backend JACK server (jackd -d dummy) — no audio hardware needed
#   2. a virtual X display (Xvfb)
#
# Both steps are idempotent: an already-running jackd / Xvfb is reused.
#
# Usage:
#   scripts/run-headless.sh                 # launch, screenshot, stop
#   SHOT=/tmp/foo.png scripts/run-headless.sh
#   KEEP_RUNNING=1 scripts/run-headless.sh  # leave the app running afterwards
#   WAIT=12 scripts/run-headless.sh         # seconds to wait before screenshot
#
# Output: a PNG at $SHOT (default /tmp/melody-shot.png). Read it to see the GUI.
# App stdout/stderr (with RUST_LOG=debug) goes to /tmp/melody-app.log.
#
# Prerequisites (Debian/Ubuntu):
#   sudo apt-get install -y jackd2 xvfb dbus-x11 imagemagick
#
set -euo pipefail

DISPLAY_NUM="${DISPLAY_NUM:-99}"
SCREEN="${SCREEN:-1600x1000x24}"   # wide enough that the control pane isn't clipped
SHOT="${SHOT:-/tmp/melody-shot.png}"
WAIT="${WAIT:-8}"
APP_LOG="${APP_LOG:-/tmp/melody-app.log}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export DISPLAY=":${DISPLAY_NUM}"
export JACK_NO_AUDIO_RESERVATION=1   # skip D-Bus device reservation in containers

log() { printf '>>> %s\n' "$*" >&2; }

# 1. Dummy JACK server -------------------------------------------------------
if jack_lsp >/dev/null 2>&1; then
	log "jackd already running"
else
	log "starting jackd (dummy backend) ..."
	# -r (global): no realtime scheduling, which is unavailable in the sandbox.
	jackd -r -d dummy -r 48000 -p 1024 >/tmp/jackd.log 2>&1 &
	for _ in $(seq 1 25); do
		jack_lsp >/dev/null 2>&1 && break
		sleep 0.2
	done
	jack_lsp >/dev/null 2>&1 || { log "ERROR: jackd failed to start (see /tmp/jackd.log)"; exit 1; }
	log "jackd up: $(jack_lsp 2>/dev/null | tr '\n' ' ')"
fi

# 2. Virtual X display -------------------------------------------------------
if pgrep -f "Xvfb :${DISPLAY_NUM}" >/dev/null 2>&1; then
	log "Xvfb already running on :${DISPLAY_NUM}"
else
	log "starting Xvfb on :${DISPLAY_NUM} (${SCREEN}) ..."
	Xvfb ":${DISPLAY_NUM}" -screen 0 "$SCREEN" >/tmp/xvfb.log 2>&1 &
	sleep 1
	pgrep -f "Xvfb :${DISPLAY_NUM}" >/dev/null 2>&1 || { log "ERROR: Xvfb failed (see /tmp/xvfb.log)"; exit 1; }
fi

# 3. Build, then launch the app ---------------------------------------------
log "building ..."
cargo build

log "launching app (RUST_LOG=debug -> ${APP_LOG}) ..."
# dbus-run-session gives GApplication a session bus to register on (otherwise it
# hangs). setsid puts the whole chain in its own process group so we can stop it
# cleanly by group id without touching cargo/build processes elsewhere.
RUST_LOG="${RUST_LOG:-debug}" setsid dbus-run-session -- \
	cargo run >"$APP_LOG" 2>&1 &
APP_PGID=$!

cleanup() {
	if [[ "${KEEP_RUNNING:-0}" == "1" ]]; then
		log "KEEP_RUNNING=1 — leaving app running (pgid ${APP_PGID})"
	else
		log "stopping app (pgid ${APP_PGID}) ..."
		kill -TERM -"${APP_PGID}" 2>/dev/null || true
	fi
}
trap cleanup EXIT

# 4. Wait for the window, then capture --------------------------------------
log "waiting ${WAIT}s for the window to map and render ..."
sleep "$WAIT"

if ! grep -q "Audio source activated" "$APP_LOG"; then
	log "WARNING: 'Audio source activated' not seen in log — app may not have started; check ${APP_LOG}"
fi

log "capturing screenshot -> ${SHOT}"
import -window root "$SHOT"

log "done. Screenshot: ${SHOT}  |  App log: ${APP_LOG}"
