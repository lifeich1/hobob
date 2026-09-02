{
  description = "An web app for hobo kicked from bibi&lili";

  inputs = {
    utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      ...
    }:
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        toolchain = pkgs.rustPlatform;
        deps = with pkgs; [
          openssl
          sqlite
        ];
      in
      rec {
        # Executed by `nix build`
        packages.default = toolchain.buildRustPackage {
          pname = "hobob";
          version = "0.1.0";
          # ⚠️ 已知限制：nix 的 flake git tree 会丢弃 gitlink，`src = ./.` 不含
          # vendor/bilibili-api-rs 源码，`nix build` 会缺依赖。
          # 曾尝试 fetchGit{submodules} 与 path 输入：相对路径都解析到 flake 的
          # store 拷贝（同样无子模块），行不通；出路见 .agents/skills/hobob-nix/SKILL.md
          #（有网时加 git 输入或 vendor 改真文件快照）。构建请用 devShell 的 cargo。
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
          buildInputs =
            with pkgs;
            [
              (with toolchain; [
                cargo
                rustc
                rustLibSrc
              ])
              clippy
              rustfmt
              pkg-config
              cargo-watch
              cargo-edit
            ]
            ++ deps;

          # Specify the rust-src path (many editors rely on this)
          RUST_SRC_PATH = "${toolchain.rustLibSrc}";

          # 经验约定（见 .agents/skills/hobob-nix/SKILL.md）：
          # - 禁止在 shellHook 里 exec（nvim/$SHELL 会吞掉 `nix develop -c <cmd>`）
          # - 自动开 Session.vim 仅限交互 tty，可用 HOBOB_NO_SESSION=1 关闭
          # - 交互开 nvim 前若系统有 zsh，把 SHELL 指向 zsh（nvim 内 :terminal/:sh 默认 shell）
          shellHook = ''
            if [ -t 0 ] && [ -z "$HOBOB_NO_SESSION" ] && [ -f Session.vim ] && command -v nvim >/dev/null 2>&1; then
              if command -v zsh >/dev/null 2>&1; then
                export SHELL="$(command -v zsh)"
              fi
              echo "[hobob devShell] opening Session.vim (set HOBOB_NO_SESSION=1 to disable)"
              nvim -S Session.vim
            fi
          '';
        };
      }
    );
}
