{
  description = "FPSDesk - a Wayland compositor navigable in first-person 3D";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          # Libraries that are dlopen()ed at runtime rather than linked, so the
          # loader has to be told where they are: winit loads libwayland and
          # libxkbcommon, Smithay loads libEGL (via libglvnd, which then picks up
          # the system Mesa from /run/opengl-driver).
          runtimeLibs = with pkgs; [
            wayland
            libxkbcommon
            libGL
          ];
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
              pkg-config
            ];

            buildInputs =
              runtimeLibs
              ++ (with pkgs; [
                # Needed once the real-session (DRM/TTY) backend lands; harmless before.
                libgbm
                libdrm
                libinput
                seatd
                udev
              ]);

            packages = with pkgs; [
              # Clients to run inside the compositor, and tools to inspect it.
              foot
              weston
              wayland-utils
              mesa-demos
              # GTK demo programs (gtk4-demo, gtk4-widget-factory): menus and
              # popovers to exercise popups and pointer input.
              gtk4.dev
            ];

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
            # The GTK demos load SVG icons through gdk-pixbuf and exit if it
            # has no SVG loader; this is where it finds one.
            GDK_PIXBUF_MODULE_FILE = "${pkgs.librsvg}/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache";
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
