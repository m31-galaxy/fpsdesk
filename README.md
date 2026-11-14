# FPSDesk

A Wayland compositor whose desktop is a 3D space you walk through in first
person. Early days: see [docs/PLAN.md](docs/PLAN.md) for the design and roadmap.

Right now it runs as a window inside your existing session and shows each
application window on a panel in a 3D scene, seen from a fixed camera. You
cannot move yet, and the mouse does not reach applications yet; typing goes to
the most recently opened window.

## Building and running

Everything needed is in the Nix flake's dev shell.

```sh
nix develop
cargo run -- -c foot                      # start FPSDesk with a terminal inside it
cargo run -- -c foot -c es2gears_wayland  # several clients, each on its own panel
cargo test
```

Programs started with `-c` connect to FPSDesk; to start one by hand, set
`WAYLAND_DISPLAY` to the socket name FPSDesk logs at startup.

Set `RUST_LOG` to change log verbosity, for example `RUST_LOG=debug` or
`RUST_LOG=info,smithay=info`.

## Checking it without looking at it

```sh
scripts/headless-run cargo run -- --smoke -c foot --screenshot /tmp/fpsdesk.png
```

`--smoke` exits 0 once a client window has been drawn for 60 frames, or 1 after
ten seconds, and `--screenshot` saves the last frame. `scripts/headless-run`
runs the command with a headless Weston as the host session, so nothing appears
on your screen and it works while the screen is locked.

## Acknowledgements

The protocol handlers started from [Smithay](https://github.com/Smithay/smithay)'s
`smallvil` example, which is MIT licensed; see
[docs/licenses/smithay-MIT.txt](docs/licenses/smithay-MIT.txt).
