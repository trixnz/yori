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

          # The Nix build sandbox does not provide a session bus or a filesystem
          # with user xattr support. The normal local gate runs these tests.
          checkFlags = [
            "--skip"
            "instance::tests::concurrent_launches_elect_exactly_one_owner"
            "--skip"
            "instance::tests::invalid_and_oversized_requests_are_rejected_before_ui_dispatch"
            "--skip"
            "instance::tests::merge_handoff_preserves_all_four_roles_until_workspace_acknowledgment"
            "--skip"
            "instance::tests::secondary_waits_for_workspace_acknowledgment_and_preserves_path_bytes"
            "--skip"
            "instance::tests::workspace_errors_are_returned_without_becoming_a_second_instance"
            "--skip"
            "cli_forwards_diff_and_merge_roles_and_exits_only_after_the_reply"
            "--skip"
            "invalid_arguments_and_missing_bus_fail_without_starting_a_window"
            "--skip"
            "storage::tests::atomic_save_preserves_source_bytes_permissions_attributes_and_old_open_handles"
          ];

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
