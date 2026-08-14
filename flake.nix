{
  inputs = {
    v_flakes.url = "github:valeratrades/v_flakes?ref=v1.6";
  };
  outputs = { self, v_flakes }:
    let
      inherit (v_flakes) flake-utils pre-commit-hooks;
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import v_flakes.default_nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
        # Canonical toolchain pinned in v_flakes — byte-identical across repos, so
        # the nix store dedups it and sccache cross-references compilations.
        rust = v_flakes.rs.default_nightly system;
        pre-commit-check = pre-commit-hooks.lib.${system}.run (v_flakes.files.preCommit { inherit pkgs; });
        manifest = (pkgs.lib.importTOML ./dockviewers/Cargo.toml).package;
        pname = manifest.name;
        stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.stdenv;

        rs = v_flakes.rs {
          inherit pkgs rust;
          build = {
            deny = false;
            workspace = let deprecate_by = "v1.0.0"; in {
              "./dockviewers_core/" = [ "git_version" "log_directives" { deprecate = { by_version = deprecate_by; force = true; }; } ];
            };
          };
        };
        github = v_flakes.github {
          inherit pkgs pname rs;
          enable = true;
          lastSupportedVersion = "nightly-2026-06-18";
          jobs.default = true;
          # The film is generated and committed, so nothing else notices when the generator moves
          # under it. Daily rather than per-run: it builds the workspace to redraw one SVG.
          jobs.other.augment = [{
            name = "asset-gate";
            args = {
              asset = "docs/.readme_assets/fuzz.svg";
              command = "nix run .#film";
              everySeconds = 86400;
            };
          }];
        };
        readme = v_flakes.readme-fw {
          inherit pkgs pname;
          defaults = true;
          lastSupportedVersion = "nightly-1.92";
          rootDir = ./.;
          badges = [ "msrv" "crates_io" "docs_rs" "loc" "ci" ];
        };
        combined = v_flakes.utils.combine { inherit rust; modules = [ rs github readme ]; };
      in
      {
        packages =
          let
            rustc = rust;
            cargo = rust;
            rustPlatform = pkgs.makeRustPlatform {
              inherit rustc cargo stdenv;
            };
          in
          {
            default = rustPlatform.buildRustPackage {
              inherit pname;
              version = manifest.version;

              buildInputs = with pkgs; [
                openssl.dev
              ];
              nativeBuildInputs = with pkgs; [ pkg-config ];

              cargoLock.lockFile = ./Cargo.lock;
              src = pkgs.lib.cleanSource ./.;
            };
          };

        # `nix run .#dev -- <framework>` boots the matching example (default: dioxus). Extra args
        # pass through to the underlying serve command, e.g. `nix run .#dev -- leptos --port 9000`.
        apps.dev = {
          type = "app";
          program = "${pkgs.writeShellScript "dev" ''
            framework="$1"; shift 2>/dev/null || true
            [ -z "$framework" ] && framework=dioxus
            case "$framework" in
              dioxus)
                exec ${pkgs.dioxus-cli}/bin/dx serve --example insilico --package dockviewers_dioxus "$@"
                ;;
              leptos)
                cd dockviewers_leptos && exec ${pkgs.trunk}/bin/trunk serve "$@"
                ;;
              *)
                echo "usage: nix run .#dev -- [dioxus|leptos] [extra serve args]" >&2
                exit 1
                ;;
            esac
          ''}";
        };

        # Redraws the README's fuzz film. The seed is pinned rather than left to the film's own
        # best-of-scan: an asset a CI job diffs has to be a function of the tree alone, and
        # best-of-scan makes it a function of the scan too.
        apps.film = {
          type = "app";
          program = pkgs.lib.getExe (pkgs.writeShellApplication {
            name = "film";
            runtimeInputs = with pkgs; [ rust git pkg-config openssl mold ];
            text = ''
              cd "$(git rev-parse --show-toplevel)"
              cargo run --example fuzz_film -p dockviewers_core -- \
                --seed 172 --out "''${ASSET_OUT:-''${1:-docs/.readme_assets/fuzz.svg}}"
            '';
          });
        };

        devShells.default =
          with pkgs;
          mkShell {
            inherit stdenv;
            shellHook =
              pre-commit-check.shellHook
              + combined.shellHook
              + ''
                cp -f ${(v_flakes.files.treefmt) { inherit pkgs; }} ./.treefmt.toml
              '';

            packages = [
              mold
              openssl
              pkg-config
              rust
              # nixpkgs dioxus-cli vendors wasm-bindgen 0.2.118 but the crate graph pins =0.2.125;
              # dx uses the matching external binary if present: `cargo binstall wasm-bindgen-cli@0.2.125`.
              dioxus-cli
              # Serves the Leptos CSR example: `nix run .#dev -- leptos`.
              trunk
            ] ++ pre-commit-check.enabledPackages ++ combined.enabledPackages;

            env.RUST_BACKTRACE = 1;
            env.RUST_LIB_BACKTRACE = 0;
            env.DIOXUS_DEVSERVER_PORT = 54580;
          };
      }
    );
}
