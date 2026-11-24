{
  description = "Aurelia — a CLI Steam launcher";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib;
        manifest = (lib.importTOML ./Cargo.toml).package;

        src = lib.cleanSourceWith {
          name = "aurelia-source";
          src = ./.;
          filter =
            path: type:
            let
              base = baseNameOf path;
            in
            !(type == "directory" && (base == "target" || base == ".git"));
        };
      in
      {
        packages.default = self.packages.${system}.aurelia;

        packages.aurelia = pkgs.rustPlatform.buildRustPackage {
          pname = "aurelia";
          inherit (manifest) version;
          inherit src;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [
            pkgs.cmake
            pkgs.perl
            pkgs.pkg-config
            pkgs.rustPlatform.bindgenHook
          ];

          buildInputs = [
            pkgs.bzip2
            pkgs.dbus
            pkgs.xz
            pkgs.zstd
          ];

          doCheck = false;

          meta = {
            inherit (manifest) description;
            homepage = "https://github.com/Drackrath/Aurelia";
            license = lib.licenses.gpl3Plus;
            mainProgram = "aurelia";
            platforms = lib.platforms.unix;
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.aurelia ];
          packages = [
            pkgs.cargo-deb
            pkgs.clippy
            pkgs.rustfmt
          ];
        };
      }
    );
}
