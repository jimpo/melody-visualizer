# Default SoundFont

`piano.sf2` is the SoundFont the MIDI generator reads when the config names no
file, or names one that fails to load.

It is made from the piano family (bank 0, programs 0–7) of
[GeneralUser GS](https://github.com/mrbumpy409/GeneralUser-GS) v2.0.3 by
S. Christian Collins (`GeneralUser-GS.sf2`, git blob
`298b552d2e9d1307e03e5c5c99d2c046aaed9ec3`). Every preset, instrument and zone
setting is kept. The audio is not: every sample is replaced by the same 64
frames of silence, keeping its root key and pitch correction. The generator
draws the fundamental from the settings and never plays a sample.

Regenerate it with

```sh
scripts/strip_soundfont.py GeneralUser-GS.sf2 assets/soundfont/piano.sf2
```

## License

GeneralUser GS's license (`LICENSE.txt`, copied here unchanged) allows use
"without restriction" and says to "feel free to use it in your software
projects, and to modify the SoundFont bank or its packaging to suit your
needs". Redistributing this stripped, modified file is within that. The license
asks not to link directly to the author's download files, which this does not.
The samples are not distributed at all.
