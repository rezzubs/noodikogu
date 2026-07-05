{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];
    forEachSystem = nixpkgs.lib.genAttrs systems;
    pkgsFor = system: nixpkgs.legacyPackages.${system};
  in {
    packages = forEachSystem (system: {
      default = (pkgsFor system).callPackage ./package.nix {};
    });

    homeModules.default = import ./module.nix;

    devShells = forEachSystem (system: {
      default = (pkgsFor system).mkShell {
        packages = with pkgsFor system; [
          cargo
          rustc
          rust-analyzer
          rustfmt
          clippy
          cargo-nextest
        ];
      };
    });

    formatter = forEachSystem (system: (pkgsFor system).alejandra);
  };
}
