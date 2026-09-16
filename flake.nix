{
  description = "yori development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    kache = {
      url = "github:kunobi-ninja/kache";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, kache }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      p4apiFor = system:
        let
          artifact = {
            x86_64-linux = {
              name = "p4api-glibc2.3-openssl3.tgz";
              hash = "sha256-JouBcM8u6tG+nvv6euJ7LgR5ZprXmmrOGusaefcNhR0=";
              platform = "bin.linux26x86_64";
            };
            aarch64-linux = {
              name = "p4api-openssl3.tgz";
              hash = "sha256-OmGjBbPvYBKxrnyvaj67J3QGxiKPgSc75g4I8l2TZRY=";
              platform = "bin.linux26aarch64";
            };
          }.${system};
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.fetchzip {
          inherit (artifact) name hash;
          url = "https://ftp.perforce.com/perforce/r25.1/${artifact.platform}/${artifact.name}";
        };
      packageFor = system:
        let
          pkgs = import nixpkgs { inherit system; };
          p4api = p4apiFor system;
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
              ./THIRD_PARTY_NOTICES.md
              ./crates
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            makeWrapper
            pkg-config
          ];
          buildInputs = runtimeLibraries ++ [ pkgs.openssl ];
          P4API_ROOT = p4api;

          # CI runs the complete suite. The package repeats only the native
          # provider tests so the pinned P4API and OpenSSL linkage is verified.
          doCheck = true;
          cargoTestFlags = [ "-p" "yori-p4" ];

          postInstall = ''
            wrapProgram "$out/bin/yori" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath (runtimeLibraries ++ [ pkgs.openssl ])}"
            install -Dm644 LICENSE "$out/share/doc/yori/LICENSE"
            install -Dm644 THIRD_PARTY_NOTICES.md "$out/share/doc/yori/THIRD_PARTY_NOTICES.md"
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
          p4api = p4apiFor system;
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
              pkg-config
            ];
            buildInputs = runtimeLibraries ++ [ pkgs.openssl ];
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibraries ++ [ pkgs.openssl ]);
            P4API_ROOT = p4api;
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
