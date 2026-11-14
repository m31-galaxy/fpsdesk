# FPSDesk

A Wayland compositor whose desktop is a 3D space you walk through in first
person. Early days: see [docs/PLAN.md](docs/PLAN.md) for the design and roadmap.
Right now it is an ordinary flat compositor that runs as a window inside your
existing session.

## Building and running

Everything needed is in the Nix flake's dev shell.

```sh
nix develop
cargo run -- -c foot              # start FPSDesk with a terminal inside it
cargo run -- -c foot -c es2gears_wayland
cargo run -- --smoke -c foot      # self-test: exits 0 once a client window has been shown for 60 frames
```

Click a window to give it keyboard focus. Programs started with `-c` connect to
FPSDesk; to start one by hand, set `WAYLAND_DISPLAY` to the socket name FPSDesk
logs at startup.

Set `RUST_LOG` to change log verbosity, for example `RUST_LOG=debug` or
`RUST_LOG=info,smithay=info`.

## Acknowledgements

The protocol handlers started from [Smithay](https://github.com/Smithay/smithay)'s
`smallvil` example, which is MIT licensed; see
[docs/licenses/smithay-MIT.txt](docs/licenses/smithay-MIT.txt).
