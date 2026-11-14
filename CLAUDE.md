# FPSDesk

A Wayland compositor navigable in first-person 3D, built on Rust and Smithay.
Read `docs/PLAN.md` before making design decisions; it records the architecture,
the milestone order, and the known risks. Update its status line when a
milestone lands.

## Environment

This is NixOS with no global toolchain. Run everything through the flake:

```sh
nix develop -c cargo build
nix develop -c cargo test
nix develop -c cargo clippy --all-targets -- -D warnings
nix develop -c cargo fmt
nix develop -c cargo run -- --smoke   # opens a window for under a second, exits 0
```

New files must be `git add`ed before Nix can see them. The dev shell provides
`foot`, `weston-terminal` and `wayland-info` as test clients.

## Conventions

- Smithay is pinned to a git revision in `Cargo.toml`. Do not bump it as a side
  effect of other work. Check APIs against that revision's source, not the
  crates.io docs, which describe a much older release.
- Raw GL calls belong in `src/render/` only, and must restore any GL state they
  change, because the context is shared with Smithay's renderer.
- `src/world/` holds no GL or Smithay types, so its logic stays unit-testable.
- Do not draft issues, comments or patches for the Smithay project: its policy
  asks contributors not to submit LLM-written material.
