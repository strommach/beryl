{
  description = "Beryl development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "aarch64-linux"; # Change to x86_64-linux if on Intel or AMD
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [
          # Rust toolchain
          rustc
          cargo
          rust-analyzer
          clippy
          rustfmt

          # C / C++ toolchain & build tools
          gcc
          clang
          cmake
          gnumake
          pkg-config
          gdb
        ];

        buildInputs = with pkgs; [
          lua5_4
          sqlite
          openssl
          xz
          zstd
        ];

        # Ensure bindgen/cc crates find C/C++ standard headers
        LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
      };
    };
}