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
```

New files must be `git add`ed before Nix can see them. The dev shell provides
`foot`, `weston-terminal`, `es2gears_wayland` (a GPU/dmabuf client) and
`wayland-info` as test clients.

## Verifying changes

Check rendering by taking a screenshot and looking at it:

```sh
nix develop -c scripts/headless-run ./target/debug/fpsdesk --smoke \
    -c "foot -e sh -c 'echo hello; sleep 5'" --screenshot <scratch>/shot.png
```

- `--smoke` exits 0 after 60 frames; with `-c <client>` the frames only count
  once that client's window has been drawn, and it exits 1 after 10 s otherwise.
- `--screenshot` saves the final frame as a PNG; open it with the Read tool.
- `scripts/headless-run` uses a headless Weston as the host session. Always use
  it for automated runs: on the user's real desktop the nested window pops up on
  their screen, and it draws no frames at all while their screen is locked, so
  every smoke run times out for reasons unrelated to the code.
- `RUST_LOG=info,smithay=warn,fpsdesk::render=trace` logs each window texture
  update, which shows whether a client's frames are arriving.

There is no way to inject input from here. Ask the user to check typing,
clicking and movement by hand, and say plainly that they are unverified.

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
