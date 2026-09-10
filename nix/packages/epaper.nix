{
  pkgs,
  pname,
  ...
}:
let
  inherit (pkgs) lib;
  # The integration test drives the CLI against tools/epdsim.py, which lives
  # outside cli/, so the source has to be assembled from both trees.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      (lib.fileset.fileFilter (file: file.hasExt "py") ../../cli)
      ../../cli/pyproject.toml
      ../../tools/epdsim.py
    ];
  };
in
pkgs.python3Packages.buildPythonApplication {
  inherit pname src;
  version = "0.1.0";
  pyproject = true;
  sourceRoot = "${src.name}/cli";

  build-system = [ pkgs.python3Packages.setuptools ];

  dependencies = with pkgs.python3Packages; [
    pyserial
    pillow
    pyte
  ];

  nativeCheckInputs = [ pkgs.python3Packages.unittestCheckHook ];
  unittestFlagsArray = [
    "-s"
    "tests"
    "-v"
  ];

  pythonImportsCheck = [ "epaper" ];

  meta = {
    description = "Host CLI for the Pico 2 driven Waveshare 7.5\" e-paper panel";
    homepage = "https://github.com/whexy/waveshare";
    mainProgram = "epaper";
    platforms = lib.platforms.unix;
  };
}
