{
  description = "Fanwaave push notification server development environment with ores-sops";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Canonical SOPS + age + env/enc/env/dec lifecycle. The exact source is
    # pinned in flake.lock so every developer and protected job uses one tool.
    ores-sops.url = "github:ORESoftware/ores-sops";
  };

  outputs = { self, nixpkgs, ores-sops, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in {
      devShells = eachSystem (system:
        let pkgs = pkgsFor system;
        in {
          default = pkgs.mkShell {
            packages = (with pkgs; [
              rustc cargo rustfmt clippy rust-analyzer
              pkg-config openssl
              git jq python3 just sops age
            ]) ++ [
              ores-sops.packages.${system}.default
            ];
            RUST_BACKTRACE = "1";
            shellHook = ''
              echo "Fanwaave push-notification-server Rust environment"
              ${ores-sops.lib.shellHook}
            '';
          };
        });
    };
}
