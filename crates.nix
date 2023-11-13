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
          buildInputs = with pkgs; [mold llvmPackages.clangUseLLVM];
        };
      };
    };
    nci.crates.npk = {};
    nci.crates.npk-build = {};
  };
}
