{
  description = "An web app for hobo kicked from bibi&lili";

  inputs = {
    utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self
    , nixpkgs
    , utils
    , ...
    }:
    utils.lib.eachDefaultSystem
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          toolchain = pkgs.rustPlatform;
          deps = with pkgs; [
            openssl
            sqlite
          ];
        in
        rec
        {
          # Executed by `nix build`
          packages.default = toolchain.buildRustPackage {
            pname = "hobob";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            buildInputs = deps;
            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            # For other makeRustPlatform features see:
            # https://github.com/NixOS/nixpkgs/blob/master/doc/languages-frameworks/rust.section.md#cargo-features-cargo-features
          };

          # Executed by `nix run`
          apps.default = utils.lib.mkApp { drv = packages.default; };

          # Used by `nix develop`
          devShells.default = pkgs.mkShell {
            buildInputs = with pkgs; [
              (with toolchain; [
                cargo
                rustc
                rustLibSrc
              ])
              clippy
              rustfmt
              pkg-config
            ] ++ deps;

            # Specify the rust-src path (many editors rely on this)
            RUST_SRC_PATH = "${toolchain.rustLibSrc}";

            shellHook = ''
              exec $SHELL
            '';
          };
        }
      );
}
