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
      basePackages = packageSet: [
        packageSet.cargo
        packageSet.clippy
        packageSet.rust-analyzer
        packageSet.rustc
        packageSet.rustfmt
      ];
      shell = packageSet: extraPackages:
        packageSet.mkShell {
          packages = basePackages packageSet ++ extraPackages;

          shellHook = ''
            export PATH="$PWD/target/release:$PATH"
          '';
        };
    in
    {
      devShells = forEachSupportedSystem (packageSet: {
        default = shell packageSet [];

        gcloud = shell packageSet [ packageSet.google-cloud-sdk ];
      });
    };
}
