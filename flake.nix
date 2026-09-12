{
  description = "yori development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
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
              ./crates
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            makeWrapper
            pkg-config
          ];
          buildInputs = runtimeLibraries;

          # The Rust CI job runs the complete test suite. This derivation only
          # verifies that the release package builds and installs correctly.
          doCheck = false;

          postInstall = ''
            wrapProgram "$out/bin/yori" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath runtimeLibraries}"
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
          pkgs = import nixpkgs { inherit system; };
          runtimeLibraries = with pkgs; [
            fontconfig
            freetype
            libxkbcommon
            vulkan-loader
            wayland
            libxcb
          ];
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              pkg-config
              dbus # isolated session buses for IPC tests
            ];
            buildInputs = runtimeLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
          };
        });
    };
}
