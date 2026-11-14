# FPSDesk

A Wayland compositor whose desktop is a 3D space you walk through in first
person. Early days: see [docs/PLAN.md](docs/PLAN.md) for the design and roadmap.

Right now it runs as a window inside your existing session. Application windows
stand on panels in an open space that you can walk around. Walk up to one, click
it, and use it with the keyboard and mouse as usual.

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
| Right click | Shoot the window under the crosshair. It flashes red and gets a bullet hole that stays 10 seconds, then fades over 8 more. A third hit while two holes still show asks the window to close (it may refuse, or ask to save first); if it does close, it shatters |

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
