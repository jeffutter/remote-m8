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
    frontend = {
      flake = true;
      url = "github:jeffutter/M8WebDisplay?rev=6d0df623deb4bef39c29b2688a63ce06967e4ab2";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
      crane,
      frontend,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        lib = nixpkgs.lib;
        craneLib = crane.mkLib pkgs;

        src = craneLib.cleanCargoSource ./.;

        envVars =
          { }
          // (lib.attrsets.optionalAttrs pkgs.stdenv.isLinux {
            RUSTFLAGS = "-Clinker=clang -Clink-arg=--ld-path=${pkgs.mold}/bin/mold";
            LD_LIBRARY_PATH = "${pkgs.alsa-lib}/lib;${pkgs.udev}/lib;${pkgs.pipewire}/lib;${pkgs.jack2}/lib;${pkgs.libopus}/lib";
            ALSA_PLUGIN_DIR = "${pkgs.pipewire}/lib/alsa-lib";
          })
          // (lib.attrsets.optionalAttrs pkgs.stdenv.isDarwin {
            # The coreaudio-sys crate is configured to look for things in whatever the
            # output of `xcrun --sdk macosx --show-sdk-path` is. However, this does not
            # always contain the right frameworks, and it uses system versions instead of
            # what we control via Nix. Instead of having to run a lot of extra scripts
            # to set our systems up to build, we can just create a SDK directory with
            # the same layout as the `MacOSX{version}.sdk` that XCode produces.
            #
            # TODO: I'm not 100% confident that this being blank won't cause issues for
            # Nix-on-Linux development. It may be sufficient to use the pkgs.symlinkJoin
            # above regardless of system! That'd set us up for cross-compilation as well.
            COREAUDIO_SDK_PATH = pkgs.symlinkJoin {
              name = "sdk";
              paths = with pkgs.darwin.apple_sdk.frameworks; [
                AudioUnit
                CoreAudio
              ];
              postBuild = ''
                mkdir -p $out/System
                mv $out/Library $out/System
              '';
            };
          });

        commonArgs = (
          {
            inherit src;

            nativeBuildInputs =
              with pkgs;
              [
                rust-bin.stable.latest.default
                cargo
                clang
                rust-analyzer
                rustc
                pkg-config
                cmake
                libopus
              ]
              ++ lib.optionals stdenv.isDarwin [
                rustPlatform.bindgenHook
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
              ++ lib.optionals stdenv.isDarwin (
                with pkgs.darwin.apple_sdk.frameworks;
                [
                  libiconv
                  AudioUnit
                  CoreAudio
                ]
              );

            preConfigurePhases = [ "copyFrontend" ];

            copyFrontend = ''
              ls -l $TEMPDIR
              ls -l $TEMPDIR/source
              mkdir -p $TEMPDIR/source/frontend
              cp -R ${frontend} $TEMPDIR/source/frontend/deploy
            '';
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
                --add-needed "${pkgs.libgcc}/lib/libgcc_s.so.1" \
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
