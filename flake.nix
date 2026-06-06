{
  description = "synth — polyphonic Rust synthesizer with hot-reloadable DSP";

  # Pinned to a tarball URL (not `github:`) so the flake resolves without
  # hitting api.github.com — mirrors thea's pin for proxy/CI-runner friendliness.
  # nixos-unstable ships Rust 1.95.0, matching rust-toolchain.toml exactly.
  # A floor is forced from below: cpal's transitive coreaudio-sys needs the
  # `edition2024` Cargo feature (Rust >=1.85) and `home@0.5.12` needs >=1.88,
  # so the 24.11 (1.82) / 25.05 (1.86) channels can't build this lock.
  inputs.nixpkgs.url = "https://github.com/NixOS/nixpkgs/archive/refs/heads/nixos-unstable.tar.gz";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in {
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          isLinux = pkgs.stdenv.isLinux;

          # Build-time tooling.
          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.makeWrapper
          ];

          # System libs the workspace links against (and dlopens at runtime).
          #   - alsa-lib      : cpal audio backend (libasound.so.2, dlopened)
          #   - libxkbcommon  : winit keyboard handling (Wayland + X11)
          #   - wayland       : eframe `wayland` feature
          #   - libGL         : eframe `glow` (OpenGL) renderer, dlopened
          #   - vulkan-loader : belt-and-braces for wgpu-style probing
          #   - xorg.lib*     : X11 fallback path for winit
          runtimeLibs = with pkgs; nixpkgs.lib.optionals isLinux [
            alsa-lib
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ];

          buildInputs = runtimeLibs;
          libPath = nixpkgs.lib.makeLibraryPath runtimeLibs;
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "synth";
            version = "0.1.0";
            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            inherit nativeBuildInputs buildInputs;

            # cargoTest tries to run the test binaries; this is a GUI/audio app
            # with no headless test deps wired, so just confirm it compiles.
            doCheck = false;

            # The synth binary uses `hot-lib-reloader`, which dlopens the
            # `synth-dsp` dylib from `target/<profile>/`. Ship the dylib next to
            # the binary and wrap it so:
            #   (1) LD_LIBRARY_PATH carries the dlopened system libs (alsa/libGL),
            #   (2) the working dir defaults are still resolvable.
            # Hot-reload itself only works from a cargo workspace (see devShell);
            # the installed binary is the static-DSP form.
            postInstall = nixpkgs.lib.optionalString isLinux ''
              # Locate the freshly built synth-dsp dylib in the cargo target tree.
              dylib=$(find target -name 'libsynth_dsp.so' | head -n1 || true)
              if [ -n "$dylib" ]; then
                install -Dm644 "$dylib" "$out/lib/libsynth_dsp.so"
              fi

              wrapProgram "$out/bin/synth" \
                --prefix LD_LIBRARY_PATH : "${libPath}:$out/lib"
            '';

            meta = with nixpkgs.lib; {
              description = "Polyphonic Rust synthesizer with hot-reloadable DSP";
              license = licenses.mit;
              platforms = platforms.linux ++ platforms.darwin;
              mainProgram = "synth";
            };
          };
        });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          isLinux = pkgs.stdenv.isLinux;
          runtimeLibs = with pkgs; nixpkgs.lib.optionals isLinux [
            alsa-lib
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
          ];
          libPath = nixpkgs.lib.makeLibraryPath runtimeLibs;
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = [
              pkgs.pkg-config
              # Toolchain matches rust-toolchain.toml (channel = 1.95.0) when the
              # host has a rustup-managed toolchain; nixpkgs rustc is the fallback.
              pkgs.rustc
              pkgs.cargo
              pkgs.rustfmt
              pkgs.clippy
              pkgs.cargo-watch
            ];
            buildInputs = runtimeLibs;

            # cpal dlopens libasound.so.2 and eframe's glow renderer dlopens
            # libGL.so.1 with no embedded RPATH — inject the nix-provided libs so
            # `cargo run -p synth` works inside `nix develop`. (Same pattern thea
            # uses for midiex's ALSA NIF.)
            shellHook = nixpkgs.lib.optionalString isLinux ''
              export LD_LIBRARY_PATH="${libPath}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              echo "synth dev shell — run ./dev.sh in one terminal, 'cargo run -p synth' in another"
            '';
          };
        });

      formatter = forAllSystems (system: (import nixpkgs { inherit system; }).nixpkgs-fmt);
    };
}
