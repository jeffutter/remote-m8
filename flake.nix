{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };
    crane = {
      url = "github:ipetkov/crane";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        lib = nixpkgs.lib;
        # craneLib = crane.lib.${system};
        craneLib = crane.mkLib pkgs;

        # src = lib.cleanSourceWith { src = craneLib.path ./.; };
        src = craneLib.cleanCargoSource ./.;

        envVars =
          { }
          // (lib.attrsets.optionalAttrs pkgs.stdenv.isLinux {
            RUSTFLAGS = "-Clinker=clang -Clink-arg=--ld-path=${pkgs.mold}/bin/mold";
            LD_LIBRARY_PATH = "${pkgs.alsa-lib}/lib;${pkgs.udev}/lib;${pkgs.pipewire}/lib;${pkgs.jack2}/lib";
            ALSA_PLUGIN_DIR = "${pkgs.pipewire}/lib/alsa-lib";
          });

        commonArgs = (
          {
            inherit src;
            nativeBuildInputs = with pkgs; [
              rust-bin.stable.latest.default
              cargo
              clang
              rust-analyzer
              rustc
              pkg-config
            ];
            buildInputs =
              with pkgs;
              [ ]
              ++ lib.optionals stdenv.isLinux [
                alsa-lib
                pipewire
                jack2
                udev
              ]
              ++ lib.optionals stdenv.isDarwin [ libiconv ];
          }
          // envVars
        );
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        bin = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
          }
          // {
            preFixup = lib.optionalString pkgs.stdenv.isLinux ''
              patchelf \
                --add-needed "${pkgs.alsa-lib}/lib/libasound.so.2" \
                --add-needed "${pkgs.udev}/lib/libudev.so.1" \
                $out/bin/remote-m8
            '';
          }
        );
      in
      with pkgs;
      {
        packages = {
          default = bin;
        };

        devShells.default = mkShell (
          commonArgs // { packages = [ ] ++ lib.optionals stdenv.isLinux [ ]; } // envVars
        );

        formatter = nixpkgs-fmt;
      }
    );
}
