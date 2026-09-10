# Linux Dependencies

```
sudo apt install build-essential clang libgtk-4-dev libjack-jackd2-dev
```

The spiral's interval ring draws its labels in
[Playfair Display](https://fonts.google.com/specimen/Playfair+Display), which
cairo finds through fontconfig. Put the font files in `~/.local/share/fonts` and
run `fc-cache`. Without the font, fontconfig draws the labels in its default
face, which on a stock Ubuntu is DejaVu Sans.
