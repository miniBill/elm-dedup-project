let
  pkgs = import (fetchTarball ("https://github.com/NixOS/nixpkgs/archive/174e938d593817f2eb5ae363684dea7c412eb96a.tar.gz")) { };
in
pkgs.buildFHSUserEnv {
  name = "elm-dedup-project";
  targetPkgs = pkgs: with pkgs; [
    # Rust dependencies
    llvmPackages_latest.llvm
    llvmPackages_latest.bintools
    llvmPackages_latest.lld
    cargo
    rustc
    rustfmt

    # Project dependencies
    pkg-config
    openssl

    # Visual Studio Code
    vscode
  ];

  extraOutputsToInstall = [ "dev" ];

  profile = ''
    export RUST_SRC_PATH="${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
  '';
  runScript = "zsh";
}
