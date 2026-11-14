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
nix develop -c cargo run -- --smoke -c foot   # self-test, see below
```

`--smoke` opens the nested window for under a second and exits 0 after 60
frames; with `-c <client>` the frames only count once that client has a window
up, and it exits 1 after 10 s otherwise. It proves a client connected and
attached a buffer, not that pixels are right or that input works: there is no
way to inject input or capture the window from here, so ask the user to check
those by hand.

New files must be `git add`ed before Nix can see them. The dev shell provides
`foot`, `weston-terminal`, `es2gears_wayland` (a GPU/dmabuf client) and
`wayland-info` as test clients.

## Commits

Do not add Claude co-author or attribution trailers to commit messages.

## Conventions

- Smithay is pinned to a git revision in `Cargo.toml`. Do not bump it as a side
  effect of other work. Check APIs against that revision's source, not the
  crates.io docs, which describe a much older release.
- Raw GL calls belong in `src/render/` only, and must restore any GL state they
  change, because the context is shared with Smithay's renderer.
- `src/world/` holds no GL or Smithay types, so its logic stays unit-testable.
- Do not draft issues, comments or patches for the Smithay project: its policy
  asks contributors not to submit LLM-written material.
