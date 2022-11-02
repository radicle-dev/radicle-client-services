{ nixpkgs ? import <nixpkgs> { }}:
let
  rust_overlay = builtins.fetchTarball "https://github.com/oxalica/rust-overlay/archive/master.tar.gz";
  pinned_pkgs = nixpkgs.fetchFromGitHub {
    owner  = "NixOS";
    repo   = "nixpkgs";
    rev    = "07875d32d5e067ffd440c5631facd0213233501a";
    sha256 = "13j941r2lkdcfcdx9n4ig5wzk21ybs9czlsihdrm0vybbx6l5f3x";
  };
  pkgs = import pinned_pkgs {
    overlays = [ (import rust_overlay ) ];
  };
  rust-bin = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain).override {
    extensions = [ "rust-src" ];
  };
in
  pkgs.mkShell {
    buildInputs = with pkgs; [
      rust-bin
      cargo-deny
      clang
      llvmPackages.libclang
      olm
      pkgconfig
      openssl
      openssl.dev
      gmp
      m4
      zlib
      cmake
      docker-compose
      dpkg
      asciidoctor
      git
    ] ++ lib.optionals stdenv.isDarwin [
      darwin.apple_sdk.frameworks.Security
      darwin.apple_sdk.frameworks.CoreServices
    ];

    LIBCLANG_PATH="${pkgs.llvmPackages.libclang}/lib";
  }
