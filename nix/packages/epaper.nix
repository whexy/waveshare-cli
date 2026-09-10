{
  pkgs,
  pname,
  ...
}:
let
  inherit (pkgs) lib;
  # The console rasterises glyphs from a real font file at runtime. Pinning it
  # into the closure keeps `epaper console` working on hosts with no fonts
  # installed; EPAPER_FONT still overrides it.
  fontPackage = pkgs.nerd-fonts.jetbrains-mono;
  fontFile =
    "${fontPackage}/share/fonts/truetype/NerdFonts/JetBrainsMono/"
    + "JetBrainsMonoNerdFontMono-Regular.ttf";
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

  nativeBuildInputs = [ pkgs.makeWrapper ];

  # A default, not a hard override: an EPAPER_FONT already in the environment
  # wins, so users can supply their own patched font.
  postFixup = ''
    wrapProgram $out/bin/epaper --set-default EPAPER_FONT ${fontFile}
  '';

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

  env.EPAPER_FONT = fontFile;

  pythonImportsCheck = [ "epaper" ];

  meta = {
    description = "Host CLI for the Pico 2 driven Waveshare 7.5\" e-paper panel";
    homepage = "https://github.com/whexy/waveshare";
    mainProgram = "epaper";
    platforms = lib.platforms.unix;
  };
}
