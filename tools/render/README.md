# Gear skin renders

Blender scripts that render the gear skins the Stage block page shows. Every skin is original. There are no maker
logos, brand scripts, artwork or signature trims.

| Script | Skins | Camera |
| --- | --- | --- |
| `pedals.py` | `od-green`, `od-gold`, `dist-black`, `fuzz`, `boost`, `comp`, `default-dark`, `default-light` | about 10° from straight down |
| `gear.py` | amps: `highgain`, `modern-us`, `british`, `chime`, `clean`, `bass`, `default-clean`, `default-crunch`, `default-hi-gain`; cabs: `cab-412-modern`, `cab-412-vintage`, `cab-212-chime`, `cab-112-clean`, `cab-810-bass`, `cab-default` | front, about 9° above |

`common.py` holds what both scripts share: the live knobs, the LCD glass rectangle, the alpha cleanup, the knob
filmstrips and the manifest.

## Build

```sh
tools/render/build_skins.sh            # every skin
tools/render/build_skins.sh british    # one skin
```

You need Blender 5.x (set `BLENDER` if it is not at the macOS default path), `cwebp` and ImageMagick. `SAMPLES` sets
the Cycles samples for the body (default 256). The knob frames use half that number.

Each skin gets a folder under `crates/ui/assets/skins/<skin>/`:

- `body.webp`: the gear on a transparent background, trimmed to the gear and its shadow (rendered 900 px tall, at most 1800 px wide). That keeps the 300-point
  stage sharp at 2×. Shadow-catcher haze below 4/255 alpha is dropped, and alpha fades to 0 over the outer 12 px.
- `knob-<control>.webp`: 32 frames of each live knob (`gain`, `output`, `mix`), from value 0 to value 1, 8 per row.
  Each frame is a square crop of the same render, feathered to a circle so that it sits on the body without a seam.
- `skin.json`: the body size, and the centre and side of each knob, in body pixels. A pedal's manifest also has
  `lcd`: the `center` and `size` of its LCD glass, in body pixels. The camera is about 10° from straight down, so the
  glass projects to a slight trapezoid; `lcd` is the axis-aligned rectangle around its top face. The glass is rendered
  blank: the app draws the active capture's name on it.

The app embeds these files. Running the build again replaces them.
