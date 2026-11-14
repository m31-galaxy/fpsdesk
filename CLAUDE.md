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
`foot`, `weston-terminal`, `es2gears_wayland` (a GPU/dmabuf client),
`gtk4-widget-factory` and `gtk4-demo` (menus, popovers, sliders) and
`wayland-info` as test clients.

## Verifying changes

Check behaviour by scripting input, taking a screenshot and looking at it:

```sh
nix develop -c scripts/headless-run ./target/debug/fpsdesk --smoke \
    -c foot --script "wait 20; use; key 35; key 23; wait 20" \
    --screenshot <scratch>/shot.png
```

- `--smoke` exits 0 after 60 frames; with `-c <client>` the frames only count
  once that client's window has been drawn, and it exits 1 after 10 s otherwise.
- `--script` plays input through the same handlers real devices use: mode
  switches, pointer moves, clicks, drags, scrolling, key taps, and camera
  moves. The steps are documented at the top of `src/script.rs`. Smoke frames
  only count once the script has finished.
- `--screenshot` saves the final frame as a PNG; open it with the Read tool.
  To click a widget, take a screenshot first and read its position off it:
  `move X Y` takes output pixels, which are screenshot pixels under
  `headless-run`.
- `--smoke-frames N` changes how many frames pass between the end of the
  script and the screenshot (default 60), to catch a short animation.
- `--camera X,Z,YAW,PITCH` (metres and degrees) starts the view somewhere else,
  for screenshots of a particular angle.
- A new window appears 1.6 m in front of wherever the camera is when it opens
  (beside that if the place is taken). To see one closer, walk to it in the
  script after it has opened, for example `walk 0 0.6`.
- FPSDesk logs to stderr, as clients do; `2> file` captures both.
- `scripts/headless-run` uses a headless Weston as the host session. Always use
  it for automated runs: on the user's real desktop the nested window pops up on
  their screen, and it draws no frames at all while their screen is locked, so
  every smoke run times out for reasons unrelated to the code.
- `RUST_LOG=info,smithay=warn,fpsdesk::render=trace` logs each window texture
  update, which shows whether a client's frames are arriving.

Verify input changes this way before handing them to the user. Keep the rules
behind input (camera, picking, movement keys, modes) in the pure `world/` and
`input/controls.rs` code, where unit tests can reach them too.

What a script cannot reach is the nested backend's handling of the host's
pointer (locking, hiding, relative motion): headless Weston has no pointer.
Changes there need the user to try them on their desktop; say so plainly.

The TTY backend (`--backend tty`) cannot be run from inside a desktop session
either. Never start it from here. Changes to `backend/tty.rs` can only be
compiled and reviewed; the user has to run them on a virtual console, starting
with a `--smoke --screenshot` run, which exits by itself.

Smithay logs `surface missing from known popups` at error level when a GTK
menu is dismissed. It comes from Smithay's own commit hook and is harmless.

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
