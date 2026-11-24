# Quick Start

This is the operator guide: how to run the application, what the keys do, what the
status bar tells you, and every configuration key. It is the only written copy of the
keyboard map and the run instructions. The in-app `F1` overlay is the other copy, and
it lives in the code. For how the application is built, see
[`README.md`](README.md) and [`docs/`](docs/).

## Build it

```
cargo build --release
```

## Run it

Pick a WSR-88D site by its four-letter ICAO identifier (e.g. `KDOX` for Dover, DE;
`KTLH` for Tallahassee, FL) and run:

```
cargo run --release -- KDOX
```

This opens a window, starts pulling live Level II data for that site from the public
real-time chunk stream on AWS S3 (`unidata-nexrad-level2-chunks`, hosted by Unidata;
see [ADR-0011](docs/adr/0011-chunk-stream-data-source.md)), and draws it as it arrives.
No account or API key is needed. The first volume can take a little while to appear as
data streams in. In the meantime, the status bar at the bottom reports what the pipeline
is doing.

Don't know a site's identifier? Any WSR-88D site works. Look one up by name on the
[NWS radar page](https://radar.weather.gov/), or try a nearby major city's airport-style
code (`K` + three letters is the usual pattern for the continental US).

No display or GPU available (e.g. over SSH, in a container)? Add `--headless` to run the
data pipeline alone and print its state transitions to the terminal instead of opening a
window:

```
cargo run --release -- KDOX --headless
```

Headless mode exits cleanly when stdin closes (Ctrl-D). To run it unattended, keep stdin
open, e.g. `tail -f /dev/null | radar-workstation KDOX --headless`. Redirecting from
`/dev/null` makes it exit immediately.

The full command line is
`radar-workstation [SITE] [--config PATH] [--headless] [--help] [--version]`. If you
leave `SITE` out, the `site` config key is used.

## Hotkeys

| Key | Action |
|---|---|
| `1`–`7` | Product: reflectivity, velocity, spectrum width, ZDR, CC, echo tops, VIL |
| `PageUp` / `PageDown` | Next / previous elevation angle (tilt). Each distinct angle is one step, so the two halves of a split cut (surveillance and Doppler at the same angle) are one tilt. VEL at the lowest tilt is there without an extra press |
| Arrow keys / left-click-drag | Pan |
| `+` / `=` / `-` / mouse wheel | Zoom (the wheel zooms toward the cursor) |
| `Home` | Reset view to site-centered, 230 km range (keeps the product/tilt selection) |
| `R` | Toggle range rings and azimuth spokes |
| `H` | Toggle primary roads overlay |
| `Space` | Play / pause the loop |
| `[` / `]` | Step back / forward one volume (stops playback; `]` past the newest volume returns to live) |
| `End` | Jump back to live |
| `,` / `.` | Loop one volume shorter / longer (2–64) |
| Click / drag the timeline row | Pin a volume / scrub through them |
| `F1` or `?` | Toggle this key list on-screen |
| `Ctrl+Q` | Quit |

## Reading the status bar

- **The mode badge** reads `LIVE`, or, filled in the timeline colour, `PAUSED` / `LOOP`
  followed by the displayed volume's full UTC time and how far it is behind live (for
  example `PAUSED 2026-11-22 21:04:12Z −18 min`). If it doesn't say `LIVE`, you are
  looking at the past.
- **Scan time** is when the volume on screen was scanned. **`age`** is how old the
  **live feed** is, measured from the live scan's own timestamp. When you are paused or
  looping it reads `live age` and still measures the live feed, not the volume on screen.
  It turns to the alert colour once the live scan is more than two nominal volume cycles
  old for its VCP (for example, 14 min for VCP 35). That means the feed is stale,
  whatever the poll readout says. Being in the past on purpose never triggers it.
- **"No VEL at 0.5° in the 21:04Z volume"** means that volume did not scan the selected
  product at the selected angle (for example, after a VCP change). Nothing from another
  volume is shown in its place. Step and play skip it, and its tick on the timeline row
  is hollow.
- **The timeline row** above the status bar appears once two or more volumes are
  retained. It has one tick per volume. The loop window is shaded, and the volume on
  screen is ringed.
- **`poll Ns ago`** shows when the last poll of the bucket succeeded. It tells you the
  network path is healthy. It does not tell you how fresh the data is.
- **Cursor readout.** Move the mouse over the radar display and the status bar shows
  range, azimuth, beam height, and the data value under the cursor.

## Configuration

The config file is `$XDG_CONFIG_HOME/radar-workstation/config`, which is usually
`~/.config/radar-workstation/config`. `--config PATH` overrides it. The format is one
`key = value` per line ([ADR-0019](docs/adr/0019-config-format.md)). The file is
optional, and a missing or broken file never stops the app from starting. A bad line or
an out-of-range value is reported as an event (in the status bar, or in the headless
log), and that key falls back to its default. Unknown keys are ignored and kept.

| Key | Type / range | Default | Written back on exit? | Owner |
|---|---|---|---|---|
| `site` | WSR-88D ICAO ID (e.g. `KDOX`) | none; the CLI argument wins | no | ADR-0019 |
| `ingest.poll_interval_seconds` | integer, clamped to 2–60 | `5` | no | ADR-0019, FR-DA-2 |
| `window.width` | integer, 640–7680 (out of range → default) | `1280` | yes | ADR-0019 |
| `window.height` | integer, 480–4320 (out of range → default) | `800` | yes | ADR-0019 |
| `view.product` | `ref`, `vel`, `sw`, `zdr`, `cc`, `echo_tops`, `vil` | `ref` | yes | ADR-0019 |
| `view.highways` | `true` / `false` (the `H` toggle) | `true` | yes | FR-DR-3 |
| `view.reference` | `true` / `false` (the `R` toggle) | `true` | yes | FR-DR-3 |
| `history.frames` | integer, 1–64 (out of range → default) | `12` | no | [ADR-0030](docs/adr/0030-volume-history-retention.md) |
| `history.budget_mb` | integer, 0–4096 (out of range → default) | `320` | no | [ADR-0030](docs/adr/0030-volume-history-retention.md) |
| `timeline.loop_frames` | integer, 2–64 (out of range → default; the `,` `.` keys) | `12` | yes | [ADR-0031](docs/adr/0031-timeline-semantics.md) |
| `timeline.frame_ms` | integer, 50–2000 ms per loop frame (out of range → default) | `250` | no | [ADR-0031](docs/adr/0031-timeline-semantics.md) |
| `timeline.dwell_ms` | integer, 0–10000 ms extra hold on the loop's last frame (out of range → default) | `1500` | no | [ADR-0031](docs/adr/0031-timeline-semantics.md) |

Values that are written back are saved only on a clean exit, and only if they changed.

**History and memory.** The app keeps a ring of recently completed volumes. The ring is
bounded by both `history.frames` and `history.budget_mb`, and the oldest volume is
dropped first. One volume costs about 28–40 MB, so the default budget holds about 8–12
volumes and adds up to 320 MB per instance. If you run several instances on a small
machine, lower `history.budget_mb`. `history.budget_mb = 0` keeps only the newest
volume, which is the Stage 5 memory footprint. Then there is no timeline row, and `Space`
says the loop needs at least two volumes. The loop plays the newest `timeline.loop_frames`
retained volumes, so it can never be longer than the history holds. Every launch starts
live, and the mode is never saved.

## What it doesn't do yet

- No backfill or archive playback. The loop covers only volumes received since launch.
- No map imagery/tiles (satellite or street-map backdrop), only vector boundaries and
  roads. This is a deliberate v1.0 scope decision, not a bug.
- No placefiles (warning polygons, storm reports, spotter/lightning overlays).
- No velocity dealiasing.
- No in-app way to change radar site. Restart with a different site argument instead.

See [`README.md`](README.md#status) for the full picture of what's implemented versus
planned.
