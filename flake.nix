{
  description = "Surveillance host and Android development environment";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { nixpkgs, ... }:
    let
      forSystems = nixpkgs.lib.genAttrs [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
      pkgsFor = system: import nixpkgs {
        inherit system;
        config = {
          allowUnfree = true;
          android_sdk.accept_license = true;
        };
      };
    in {
      devShells = forSystems (system:
        let
          pkgs = pkgsFor system;
          hostTools = with pkgs; [ cargo rustc rustfmt clippy ffmpeg gh pkg-config python3 ];
          sdk = (pkgs.androidenv.composeAndroidPackages {
            platformVersions = [ "35" ];
            buildToolsVersions = [ "35.0.0" ];
            includeEmulator = false;
            includeSystemImages = false;
            includeNDK = false;
          }).androidsdk;
        in {
          default = pkgs.mkShell { packages = hostTools; };
          android = pkgs.mkShell {
            packages = [ pkgs.jdk17 sdk ];
            JAVA_HOME = "${pkgs.jdk17}";
            ANDROID_HOME = "${sdk}/libexec/android-sdk";
            ANDROID_SDK_ROOT = "${sdk}/libexec/android-sdk";
            shellHook = ''
              export PATH="$ANDROID_HOME/platform-tools:$PATH"
            '';
          };
        });
      packages = forSystems (system:
        let
          pkgs = pkgsFor system;
          host = pkgs.rustPlatform.buildRustPackage {
            pname = "surveillance";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.rcodesign ];
            postInstall = nixpkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
              app="$out/Applications/Surveillance.app/Contents"
              mkdir -p "$app/MacOS"
              cp packaging/Info.plist "$app/Info.plist"
              mv "$out/bin/surveillance" "$app/MacOS/surveillance"
              ln -s "$app/MacOS/surveillance" "$out/bin/surveillance"
            '';
            postFixup = nixpkgs.lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
              rcodesign sign "$out/Applications/Surveillance.app"
            '';
            SURVEILLANCE_BUILD_FFMPEG = "${pkgs.ffmpeg}/bin/ffmpeg";
          };
        in { default = host; });
    };
}
