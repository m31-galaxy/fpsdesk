# FPSDesk

A Wayland compositor whose desktop is a 3D space you walk through in first
person. Early days: see [docs/PLAN.md](docs/PLAN.md) for the design and roadmap.

## Building

Everything needed is in the Nix flake's dev shell.

```sh
nix develop
cargo run              # opens a nested window on your current Wayland session
cargo run -- --smoke   # same, but exits by itself after 60 frames
```

Set `RUST_LOG` to change log verbosity, for example `RUST_LOG=debug` or
`RUST_LOG=info,smithay=info`.
