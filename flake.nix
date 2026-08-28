{
  description = "A minimalist, secure engineering primitive for agentic SSH execution and detached background operations";

  nixConfig = {
    extra-substituters = [
      "https://sandbanks.cachix.org"
    ];
    extra-trusted-public-keys = [
      "sandbanks.cachix.org-1:4OivlISgqyRf860wy5yXPcvVYzxrR1aYf/C4gWR5z4c="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        agentic_ssh-pkg = pkgs.rustPlatform.buildRustPackage {
          pname = "agentic_ssh";
          version = "0.4.10";
          src = ./.;

          cargoHash = "sha256-FOHtjqeO1DGRnf6PaHgPDKvAUKm4UkJHWHvYO15CsxQ=";

          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.apple-sdk_15
            pkgs.libiconv
          ];

          nativeBuildInputs = [ pkgs.pkg-config ];

          # Unit tests for CLI/integration requiring network/SSH are skipped in Nix sandbox
          doCheck = false;
        };
      in
      {
        packages.default = agentic_ssh-pkg;
        packages.agentic_ssh = agentic_ssh-pkg;

        apps.default = {
          type = "app";
          program = "${agentic_ssh-pkg}/bin/agentic_ssh";
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ agentic_ssh-pkg ];
          packages = with pkgs; [
            rustfmt
            clippy
            cargo
            rustc
          ];
        };
      }
    );
}
