{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    pre-commit-hooks.url = "github:cachix/git-hooks.nix";
    pre-commit-hooks.inputs.nixpkgs.follows = "nixpkgs";
    v_flakes.url = "github:valeratrades/v_flakes?ref=v1.6";
    v_flakes.inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { self, nixpkgs, flake-utils, pre-commit-hooks, v_flakes }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          allowUnfree = true;
        };
        # Canonical toolchain pinned in v_flakes — byte-identical across repos, so
        # the nix store dedups it and sccache cross-references compilations.
        rust = v_flakes.rs.default_nightly system;
        pre-commit-check = pre-commit-hooks.lib.${system}.run (v_flakes.files.preCommit { inherit pkgs; });
        manifest = (pkgs.lib.importTOML ./dockviewers_dioxus/Cargo.toml).package;
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
                exec ${pkgs.trunk}/bin/trunk serve dockviewers_leptos/index.html "$@"
                ;;
              *)
                echo "usage: nix run .#dev -- [dioxus|leptos] [extra serve args]" >&2
                exit 1
                ;;
            esac
          ''}";
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
