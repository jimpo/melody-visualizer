#!/usr/bin/env bash
#
# Drive the running melody-visualizer with a JACK sine tone, so the live
# audio → spectrum → graphic pipeline can actually be observed: the spiral
# lights up a bright band at the tone's frequency (a pure sine → one band).
#
# Prerequisites:
#   - a JACK server (the dummy one from scripts/run-headless.sh is fine)
#   - the app already running, e.g.:
#         KEEP_RUNNING=1 scripts/run-headless.sh
#   - jack_simple_client / jack_connect (the jackd2 example tools)
#
# Usage:
#   scripts/connect-test-tone.sh                 # connect a sine to the app
#   scripts/connect-test-tone.sh --disconnect    # stop driving the input
#
# After connecting, screenshot the framebuffer to see the lit band:
#   DISPLAY=:99 import -window root /tmp/melody-tone.png
#
set -euo pipefail

export JACK_NO_AUDIO_RESERVATION=1

INPUT="${INPUT:-Melody Visualizer:input}"   # the app's registered input port
CLIENT="${CLIENT:-tone}"                     # jack_simple_client client name

log() { printf '>>> %s\n' "$*" >&2; }

if [[ "${1:-}" == "--disconnect" ]]; then
	jack_disconnect "${CLIENT}:output1" "$INPUT" 2>/dev/null || true
	log "disconnected ${CLIENT}:output1 from ${INPUT}"
	exit 0
fi

if ! jack_lsp 2>/dev/null | grep -qx "${INPUT}"; then
	log "ERROR: '${INPUT}' not found — is the app running? (KEEP_RUNNING=1 scripts/run-headless.sh)"
	exit 1
fi

# jack_simple_client emits a steady sine on <name>:output1 / output2.
if ! jack_lsp 2>/dev/null | grep -qx "${CLIENT}:output1"; then
	log "starting sine generator (jack_simple_client ${CLIENT}) ..."
	setsid jack_simple_client "$CLIENT" >/tmp/jack_tone.log 2>&1 &
	for _ in $(seq 1 25); do
		jack_lsp 2>/dev/null | grep -qx "${CLIENT}:output1" && break
		sleep 0.2
	done
fi
jack_lsp 2>/dev/null | grep -qx "${CLIENT}:output1" || { log "ERROR: sine generator did not start (see /tmp/jack_tone.log)"; exit 1; }

log "connecting ${CLIENT}:output1 -> ${INPUT}"
jack_connect "${CLIENT}:output1" "$INPUT" 2>/dev/null || true
log "connected — the spiral should now light a band at the tone's frequency."
