{
  description = "yori development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    kache = {
      url = "github:kunobi-ninja/kache";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, kache }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      packageFor = system:
        let
          pkgs = import nixpkgs { inherit system; };
          runtimeLibraries = with pkgs; [
            fontconfig
            freetype
            libxkbcommon
            vulkan-loader
            wayland
            libxcb
          ];
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "yori";
          version = "0.1.0";

          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.lock
              ./Cargo.toml
              ./LICENSE
              ./assets
              ./crates
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            makeWrapper
            perl
            pkg-config
          ];
          buildInputs = runtimeLibraries;

          # CI runs the complete suite. The package repeats the Perforce
          # provider tests without requiring a live server or installed CLI.
          doCheck = true;
          cargoTestFlags = [ "-p" "yori-p4" ];

          postInstall = ''
            wrapProgram "$out/bin/yori" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath runtimeLibraries}"
            install -Dm644 LICENSE "$out/share/doc/yori/LICENSE"
            install -Dm644 assets/platform/linux/io.github.trixnz.yori.desktop \
              "$out/share/applications/io.github.trixnz.yori.desktop"
            install -Dm644 assets/app-icon.png \
              "$out/share/icons/hicolor/1024x1024/apps/io.github.trixnz.yori.png"

            for icon in assets/platform/linux/hicolor/*/apps/*.png; do
              size="$(basename "$(dirname "$(dirname "$icon")")")"
              install -Dm644 "$icon" \
                "$out/share/icons/hicolor/$size/apps/$(basename "$icon")"
            done
          '';

          meta = {
            description = "Native source-file diff editor";
            homepage = "https://github.com/trixnz/yori";
            license = pkgs.lib.licenses.mit;
            mainProgram = "yori";
            platforms = systems;
          };
        };
    in {
      packages = forAllSystems (system:
        let package = packageFor system;
        in {
          default = package;
          yori = package;
        });

      apps = forAllSystems (system:
        let
          app = {
            type = "app";
            program = "${self.packages.${system}.yori}/bin/yori";
            meta.description = "Compare and merge source files";
          };
        in {
          default = app;
          yori = app;
        });

      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          runtimeLibraries = with pkgs; [
            fontconfig
            freetype
            libxkbcommon
            vulkan-loader
            wayland
            libxcb
          ];
          shellAttributes = {
            nativeBuildInputs = with pkgs; [
              rustToolchain
              perl
              pkg-config
              (python3.withPackages (pythonPackages: [ pythonPackages.pillow ]))
            ];
            buildInputs = runtimeLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
          };
          kachePackage = kache.packages.${system}.kache;
        in {
          default = pkgs.mkShell shellAttributes;
          kache = pkgs.mkShell (shellAttributes // {
            packages = [ kachePackage ];
            RUSTC_WRAPPER = "${kachePackage}/bin/kache";
          });
        });
    };
}
