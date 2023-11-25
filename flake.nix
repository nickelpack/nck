{
  description = "Nickelpack Package Manager";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nci = {
      url = "github:yusdacra/nix-cargo-integration";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {flake-parts, ...}:
    flake-parts.lib.mkFlake {inherit inputs;} {
      imports = [
        inputs.nci.flakeModule
        ./crates.nix
      ];
      systems = ["x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin"];
      perSystem = {
        config,
        self',
        inputs',
        pkgs,
        system,
        ...
      }: let
        outputs = config.nci.outputs;
      in {
        # Per-system attributes can be defined here. The self' and inputs'
        # module parameters provide easy access to attributes of the same
        # system.

        packages.default = outputs.nip.packages.release;
        devShells.default = outputs.nickelpack.devShell.overrideAttrs (old: {
          packages = with pkgs; (old.packages or []) ++ [cargo-expand gdb cargo-udeps];
          NPK__LINUX__RUNTIME_DIR = "/tmp/npk";
          RUST_LOG = "info,npk_sandbox=trace";
          shellHook = ''
            declare -a parts
            try_find() {
              id=$1
              fn=$2
              parts=( )
              if line=$(cat "$fn" | grep -E "^$id:[0-9]+:[0-9]+\$" | grep -oE '[0-9]+:[0-9]+$'); then
                IFS=':' read -r -a parts <<< "$line"
                start="''${parts[0]}"
                length="''${parts[1]}"
                end=$(($start + $length))
                echo "$start $end"
                return 0
              fi
              return 1
            }

            if user=$(try_find $(id -u) /etc/subuid) || user=$(try_find $(id -un) /etc/subuid); then
              parts=( )
              IFS=' ' read -r -a parts <<< "$user"
              export NPK__LINUX__ID_MAP__UID_MIN=''${parts[0]}
              export NPK__LINUX__ID_MAP__UID_MAX=''${parts[1]}
            fi

            if group=$(try_find $(id -g) /etc/subgid) || group=$(try_find $(id -gn) /etc/subgid); then
              parts=( )
              IFS=' ' read -r -a parts <<< "$group"
              export NPK__LINUX__ID_MAP__GID_MIN=''${parts[0]}
              export NPK__LINUX__ID_MAP__GID_MAX=''${parts[1]}
            fi
          '';
        });
        formatter = pkgs.alejandra;
      };
      flake = {
        # The usual flake attributes can be defined here, including system-
        # agnostic ones like nixosModule and system-enumerating ones, although
        # those are more easily expressed in perSystem.
      };
    };
}
