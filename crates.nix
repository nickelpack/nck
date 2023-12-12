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
    nci.crates.nck = {};
    nci.crates.nck-daemon = {};
    nci.crates.nck-sandbox = {};
    nci.crates.nck-core = {};
  };
}
