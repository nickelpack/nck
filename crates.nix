{...}: {
  perSystem = {
    pkgs,
    config,
    ...
  }: {
    nci.projects.nickelpack = {
      path = ./.;
      export = true;
      drvConfig = {
        mkDerivation = {
          buildInputs = with pkgs; [llvmPackages.clangUseLLVM llvmPackages.bintools cargo-nextest];
        };
      };
    };
    nci.crates.npk = {};
    nci.crates.npk-daemon = {};
    nci.crates.npk-sandbox = {};
    nci.crates.npk-util = {};
  };
}
