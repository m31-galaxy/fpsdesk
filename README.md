# FPSDesk

A Wayland compositor whose desktop is a 3D space you walk through in first
person. Early days: see [docs/PLAN.md](docs/PLAN.md) for the design and roadmap.

It runs either as a window inside your existing session or, started from a
virtual console, as a session of its own. Application windows stand on panels
in an open space that you can walk around. Walk up to one, click it, and use it with the
keyboard and mouse as usual.

## Building and running

Everything needed is in the Nix flake's dev shell.

```sh
nix develop
cargo run -- -c foot                      # start FPSDesk with a terminal inside it
cargo run -- -c foot -c gtk4-widget-factory
cargo test
```

Programs started with `-c` connect to FPSDesk; to start one by hand, set
`WAYLAND_DISPLAY` to the socket name FPSDesk logs at startup.

## Starting from a TTY

Switch to a virtual console (Ctrl+Alt+F3, say), log in, and run it there, with
no desktop running on that console. It needs a seat manager (seatd or logind)
and `XDG_RUNTIME_DIR` set, which a normal login provides.

```sh
nix develop -c cargo build
# A first check: draws for about a second, saves a picture, and exits by itself.
nix develop -c ./target/debug/fpsdesk --backend tty --smoke --screenshot /tmp/fpsdesk.png 2> /tmp/fpsdesk.log
# A first interactive try that quits by itself after 30 seconds, in case the
# keyboard does not respond:
nix develop -c ./target/debug/fpsdesk --backend tty --exit-after 30 -c foot 2> /tmp/fpsdesk.log
# The real thing:
nix develop -c ./target/debug/fpsdesk --backend tty -c foot 2> /tmp/fpsdesk.log
```

`--backend` defaults to `nested` when `WAYLAND_DISPLAY` or `DISPLAY` is set and
to `tty` otherwise. Logs go to stderr, so redirect them as above. `--scale 2`
doubles the size of everything on a HiDPI panel. FPSDesk draws the world for one
monitor, the internal panel if there is one, and mirrors that picture (scaled to
fit, with black bars) onto any other connected monitor, including ones plugged in
while it runs. Mirroring has not been run on real hardware yet.

| | |
| --- | --- |
| **Ctrl+Alt+F1** to **F12** | Switch to that virtual console (works in both modes) |
| **Ctrl+Alt+Backspace** | Quit FPSDesk (works in both modes) |

Switching away and back pauses and restores FPSDesk. With no host cursor, a
built-in arrow is drawn in interact mode, and tap-to-click is turned on for
touchpads.

## Controls

FPSDesk starts in walk mode and takes hold of the mouse.

| | |
| --- | --- |
| **Ctrl+Escape** | Switch between walk and interact mode |

Walk mode, with a crosshair in the middle of the view:

| | |
| --- | --- |
| W A S D, arrows | Walk |
| Shift | Run |
| Mouse | Look around |
| Left click, Enter | Focus the window under the crosshair (outlined in blue) and switch to interact mode |
| `, / | Open the launcher prompt at the bottom of the view |
| Right click | Shoot the window under the crosshair. It flashes red and gets a bullet hole that stays 10 seconds, then fades over 8 more. A third hit while two holes still show asks the window to close (it may refuse, or ask to save first); if it does close, it shatters |

The launcher takes every key while it is open: type a command (run through
`sh -c`, as a client of FPSDesk), Enter runs it, Backspace edits, Up and Down
recall earlier commands, Escape closes it without running anything. It is not
available in interact mode, where the keys belong to the application.

Interact mode, with a mouse cursor:

| | |
| --- | --- |
| Keyboard | Goes to the focused window (outlined in amber) |
| Mouse | Clicks, drags and scrolls whatever window is under the cursor (right click opens context menus; it does not shoot); clicking a window focuses it |
| Click on nothing | Back to walk mode, like Ctrl+Escape |

Super also switches modes, but a host desktop normally keeps that key for
itself.

## Logging

Set `RUST_LOG` to change log verbosity, for example `RUST_LOG=debug` or
`RUST_LOG=info,smithay=info`. `RUST_LOG=info,smithay=warn,fpsdesk::frame_stats=debug`
reports frame times every five seconds.

## Checking it without looking at it

```sh
scripts/headless-run cargo run -- --smoke -c foot \
    --script "wait 20; use; key 35; key 23; wait 20" --screenshot /tmp/fpsdesk.png
```

- `--smoke` exits 0 once a client window has been drawn for 60 frames (or
  `--smoke-frames N`, to catch a short animation), or 1
  after ten seconds.
- `--script` plays input back through the same code paths as real devices:
  here it focuses the terminal and types "hi". The steps are listed at the top
  of [src/script.rs](src/script.rs).
- `--screenshot` saves the last frame, after the script has finished.
- `--camera X,Z,YAW,PITCH` sets where the view starts.
- `scripts/headless-run` runs the command with a headless Weston as the host
  session, so nothing appears on your screen and it works while the screen is
  locked.

## Acknowledgements

The protocol handlers started from [Smithay](https://github.com/Smithay/smithay)'s
`smallvil` example, which is MIT licensed; see
[docs/licenses/smithay-MIT.txt](docs/licenses/smithay-MIT.txt).
