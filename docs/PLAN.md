# FPSDesk plan

FPSDesk is a Wayland compositor whose desktop is a 3D space you move through in
first person. Application windows are panels standing in that space.

Status: **M0 to M3 done. M4 implemented**: clicking, dragging, scrolling,
typing, menus and client cursors are checked by scripted input and
screenshots. What is left to confirm by hand is the nested window's handling
of the host's own pointer when switching modes. **The TTY backend of M6 is
written but has never been run on a real console** (see M6); a login-session
entry and NixOS module are still to do. Next up is M5.

## Decisions

| Question | Decision |
| --- | --- |
| Stack | Rust + [Smithay](https://github.com/Smithay/smithay), pinned to a git revision |
| Graphics | OpenGL ES 3 through Smithay's `GlesRenderer`, with our own 3D pass in the same context |
| Ambition | Staged: nested window first, then a real login session |
| World | Open space with free-standing panels first; loadable maps later |
| Controls | Modal: **walk** (WASD + mouselook) and **interact** (keyboard and pointer go to an app) |
| Dev environment | Nix flake dev shell, pinned to the system's nixpkgs revision |

Still open (none of these block M1-M3):

- **Licence.** Smithay is MIT; pick one before publishing.
- **Multi-monitor meaning.** One wide view spanning outputs, or one output for
  the world and others for flat "pinned" windows? Decide in M6.
- **Decorations.** Ask clients for server-side decorations and draw a thin 3D
  frame, or let clients draw their own? Decide in M5.

## Architecture

### The core trick

Smithay does the Wayland work: protocols, input devices, buffer import. Every
client buffer ends up as a `GlesTexture`, and Smithay lets us run raw GL in its
context (`GlesRenderer::with_context` hands over the `ffi::Gles2` bindings).
What Smithay does not have is any notion of 3D; its renderer and `Space` are
strictly 2D. So the frame is built in three passes:

1. **Window pass (2D, Smithay).** Each window's surface tree (toplevel plus
   subsurfaces) is composited into its own offscreen texture with Smithay's
   normal render elements and damage tracking. Only damaged windows re-render.
2. **World pass (3D, ours).** The scene is drawn into our own framebuffer with a
   depth buffer: sky, ground, and one textured quad per window, sampling the
   textures from pass 1.
3. **Present pass (2D, Smithay).** The world framebuffer's colour texture is
   wrapped as a Smithay texture (`GlesTexture::from_raw`) and shown as a single
   full-screen render element, with screen-space overlays on top: crosshair,
   cursor, HUD, and later lock screen and layer-shell surfaces.

Why the extra copies rather than drawing client buffers straight onto quads:

- Pass 1 lets Smithay's tested code handle subsurfaces, viewporter crops, buffer
  transforms and scales, and external-only (YUV video) textures. We always get a
  plain RGBA texture.
- A plain texture that we own can have **mipmaps and anisotropic filtering**,
  which is what makes text readable at a distance or an angle. Client buffers
  cannot be mipmapped in place.
- Pass 3 makes the world independent of the backend. The same code presents to a
  nested window now and to DRM later, and the world can render at a different
  resolution from the output.

### Modules

Single crate until something forces a split.

```
src/
  main.rs       argument parsing, logging, event loop
  state.rs      compositor state: Smithay protocol states, seat, windows
  handlers/     Wayland protocol handler impls (xdg-shell, compositor, shm, ...)
  backend/      winit.rs (nested); tty.rs (libseat, DRM/GBM, libinput)
  frame.rs      the per-frame work both backends share
  input/        mode state machine, key bindings, pointer routing
  script.rs     scripted input for automated checks (--script)
  world/        camera, panels, ray picking, placement. No GL, no Smithay types.
  render/       all raw GL: world framebuffer, shaders, window textures
```

Two boundaries to hold on to:

- `world/` stays pure maths and data so it can be unit tested without a GPU or a
  Wayland connection. Ray-plane intersection, the mode machine and placement
  rules are where the subtle bugs will be, and they are all testable this way.
- Raw GL lives only in `render/`. Smithay's contract for `with_context` is that
  we restore any GL state we touch, so that code should sit behind one guard
  rather than be scattered.

### Panels and picking

A window in the world is a **panel**: a position, an orientation, and a physical
size derived from its pixel size by a pixels-per-metre density. One function
underlies all pointer interaction:

> screen point -> ray from camera -> nearest panel hit -> (window, surface-local x/y)

- In **walk** mode the screen point is the centre (the crosshair) and mouse
  motion turns the camera.
- In **interact** mode the camera is still, mouse motion moves a screen-space
  cursor, and the hit result is sent to the client as ordinary pointer events.

Because both modes share that function, hover highlighting, click-to-focus,
popups and drag all come from the same code path.

### Control modes

```
          click or Enter on a panel
  WALK  ---------------------------->  INTERACT
        <--------------------------->
           Ctrl+Escape or Super (compositor-reserved; toggles)
```

- **Walk:** WASD moves, mouse looks, crosshair highlights the panel it is on. No
  client has keyboard focus. Compositor bindings (launch, close, grab) live here.
- **Interact:** the keyboard belongs to the focused client. On entry the camera
  can ease to face the panel squarely at a distance where it is comfortably
  readable. The reserved binding returns to walk mode; everything else reaches
  the app. Entering it with nothing under the crosshair just frees the mouse.
- **The reserved binding** is Super in a real session. Nested under a desktop,
  the host keeps Super for itself, so Ctrl+Escape does the same job.
- **Grab** (M5) is a sub-mode of walk: the panel under the crosshair follows the
  camera so it can be carried, pushed, pulled, rotated and resized.

## Milestones

Each one ends in something runnable.

**M0 - Preparation.** Done. Flake dev shell; Smithay pinned and compiling;
`cargo run -- --smoke` opens a nested window with hardware GLES 3.2 and exits.

**M1 - Flat baseline.** A minimal ordinary compositor, nested: `wl_compositor`,
`xdg_shell`, `wl_shm`, dmabuf, `wl_seat`, an output. Windows drawn flat with
Smithay's stock pipeline. Smithay's `smallvil` example (about 1,400 lines) is the
template. *Done when* `foot` runs inside and accepts typing.

**M2 - First quad.** The key technical spike: the three-pass pipeline with a
fixed camera. *Done when* a live `foot` window is visible in perspective on a
quad and keeps updating. This proves the whole rendering approach; if something
is wrong with it, we find out here, cheaply.

**M3 - Walk.** Camera, WASD, mouselook, ground grid, sky, crosshair, hover
highlight by ray picking. Includes solving relative pointer motion in nested
mode (see Risks). *Done when* you can walk around several windows.

**M4 - Interact.** The mode machine, focus routing, pointer events from picking,
scroll, cursor image, popups and menus. *Done when* a terminal and a GTK app
with menus are properly usable from inside the world.

**M5 - Spatial window management.** Where new windows spawn; grab, move, rotate,
resize, close; a launcher binding; a config file; the legibility work
(anisotropic filtering, antialiased panel edges, snap-to-face on focus, client
render scale); dithering to hide banding in the dark ground and sky gradients.
*Done when* the nested build is a complete, pleasant demo.

**M6 - Real session.** udev/DRM/libinput/libseat backend, VT switching, vblank-
driven frame timing, a session entry so it can be chosen at login, a NixOS
module. *Done when* it runs from a TTY on this laptop.

The backend part is written (`backend/tty.rs`, `--backend tty`) but untested
on hardware: single GPU, one output (the internal panel if there is one, at
its preferred mode, others ignored), `GbmBufferedSurface` driven by vblank,
libinput with tap-to-click, a built-in arrow cursor, Ctrl+Alt+F1..F12 and
Ctrl+Alt+Backspace always handled first, pause and resume on VT switches, and
any fatal error exiting cleanly. A dmabuf target is drawn with
`Transform::Normal` (first row at the top), a window surface with
`Flipped180`; the screenshot code follows the same rule. Still to do: the
login-session entry, the NixOS module, monitor hot-plug, a decision on what
several monitors mean, and output scale chosen automatically (`--scale` for
now).

**M7 - Desktop plumbing.** In rough priority order: clipboard and primary
selection; X11 apps (try `xwayland-satellite` first, which runs X11 as an
ordinary Wayland client and avoids writing an X window manager); xdg-decoration;
fractional-scale; layer-shell (as screen-space HUD); session-lock and idle;
pointer-constraints and relative-pointer (for games); screen capture and portals;
xdg-activation; input methods. Smithay already implements the protocol side of
all of these; the work is deciding what each means in 3D.

**M8 - Loadable maps.** glTF levels; collision against level geometry; named
anchor surfaces where windows can hang; lighting; switching maps. Rooms can then
play the role of workspaces. Start unlit with baked textures and add lighting
only if it earns its cost.

**M9 - Polish.** Layout persistence per app, gamepad input, spatial audio, and
whatever using it day to day shows up.

## Risks

1. **Legibility and comfort.** The biggest risk is not technical: text on a
   perspective-projected surface is harder to read than a flat desktop, and
   mouselook can be tiring. Mitigations are in M5, but treat "is this actually
   nice to use for an hour?" as the question M3-M5 exist to answer. If the
   answer is no, interact mode can fall back to showing the focused window flat
   and pixel-perfect.
2. **Mouselook in nested mode.** Smithay's winit backend reports only absolute
   pointer positions (`PointerMotionEvent = UnusedEvent`) and never locks the
   host cursor. Handled in M3 without patching Smithay: the pointer is locked
   and hidden through winit's own window API, and
   `backend/relative_pointer.rs` opens a second event queue on winit's
   connection to the host to receive relative-pointer events directly. On a
   host without that protocol (X11, headless Weston) mouselook falls back to
   differences between absolute positions and stops at the window's edge. The
   DRM backend has no such problem, since libinput is relative by nature.
3. **Sharing GL state with Smithay.** The renderer's GL state is an
   implementation detail we must leave as we found it. Contained by the
   `render/` boundary and by pass 2 drawing only into our own framebuffer.
4. **Smithay churn.** We track git master, which has no stability promise.
   The revision is pinned in `Cargo.toml`; upgrade on purpose, not incidentally.
5. **Popups in 3D.** xdg-popup positioning assumes a screen rectangle to stay
   within. Handled in M4: a window's texture and quad grow to take in its open
   popups, which lie in the window's plane and may hang past its edges, and
   popups are only constrained to a generous margin around the window. Picking
   treats the gaps between a window and its popups as holes.
6. **Performance on integrated graphics.** Regenerating mipmaps for large,
   busy windows each frame looked like it might be costly. Measured in M3 on
   this laptop (Intel Arc B390) with a constantly scrolling 2560x1600 terminal
   and `glFinish` after every frame: about 3.9 ms per frame with mipmaps
   against 3.5 ms without, and 0.7 ms for an empty scene. Mipmaps are not the
   problem; most of the cost is uploading the window's pixels. That was with a
   1280x800 output, so re-measure at the panel's full resolution in M6. The
   8.3 ms budget of a 120 Hz display leaves room. `RUST_LOG=fpsdesk::frame_stats=debug`
   logs frame times (CPU side only).
7. **Nested mode stalls when hidden.** The nested backend redraws only when
   the host asks for a frame, and a desktop stops asking while the window is
   hidden or the screen is locked, which also freezes the clients inside.
   Harmless for development, but automated checks must run under
   `scripts/headless-run`. The DRM backend paces itself and is unaffected.

## Working with upstream

Smithay's contribution policy (`AI.md` in their repository) requires that
contributors fully understand their patches and disclose AI assistance, and
strongly advises against LLM-written contributions, issues and comments. Any
change we need in Smithay should be carried as a local patch; if it is worth
upstreaming, write and submit it by hand.

## References

- Smithay `smallvil` and `anvil` - minimal and full example compositors.
- niri, cosmic-comp - production Smithay compositors; good models for the
  udev backend, offscreen rendering and protocol coverage.
- Stardust XR, Simula, SafeSpaces, Motorcar - earlier 3D and XR window
  environments, worth reading for interaction ideas and pitfalls.
