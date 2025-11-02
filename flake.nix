{
  description = "Kubernetes operator for managing RustFS storage clusters";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };

        nativeBuildInputs = with pkgs; [
          rustup
          pkg-config
          just
          cargo-nextest
          cargo-watch
        ];

        buildInputs = with pkgs; [
          openssl
          zlib
          nil
          docker
          docker-buildx
          kubernetes-helm
          kubectl

          # For testing
          minio-client
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;
          name = "rustfs-operator";
        };
      }
    );
}
