{
  description = "tog development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { nixpkgs, ... }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];
      forEachSupportedSystem = function:
        nixpkgs.lib.genAttrs supportedSystems (system:
          function (import nixpkgs { inherit system; }));
    in
    {
      devShells = forEachSupportedSystem (packageSet: {
        default = packageSet.mkShell {
          packages = [
            packageSet.cargo
            packageSet.clippy
            packageSet.rust-analyzer
            packageSet.rustc
            packageSet.rustfmt
          ];

          shellHook = ''
            export PATH="$PWD/target/release:$PATH"
          '';
        };
      });
    };
}
