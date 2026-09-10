{
  inputs,
  pkgs,
  perSystem,
  ...
}:
let
  # TinyUSB and friends live in the SDK submodules; the plain derivation omits
  # them and USB support silently disappears from the CMake build.
  picoSdk = pkgs.pico-sdk.override { withSubmodules = true; };
  python = pkgs.python3.withPackages (ps: [
    ps.pyserial
    ps.pillow
    ps.pyte
  ]);
  pre-commit-check = import ./checks/pre-commit-check.nix { inherit inputs pkgs; };
  consoleFont =
    "${pkgs.nerd-fonts.terminess-ttf}/share/fonts/truetype/NerdFonts/"
    + "Terminess/TerminessNerdFontMono-Regular.ttf";
in
pkgs.mkShell {
  packages = [
    pkgs.ffmpeg # AVFoundation frame grab from the OBS virtual camera
    pkgs.imagemagick # crop/deskew/threshold the captured panel
    pkgs.coreutils # `timeout --signal=KILL`; macOS ships no BSD equivalent
    pkgs.jq

    pkgs.cmake
    pkgs.ninja
    pkgs.gcc-arm-embedded
    pkgs.picotool
    picoSdk
    python
    pkgs.mpremote # talks to the MicroPython REPL still on the board

    perSystem.self.epaper # the packaged CLI, alongside the source checkout
  ];

  env.PICO_SDK_PATH = "${picoSdk}/lib/pico-sdk";

  # Running the checkout with `python -m epaper` bypasses the packaged
  # wrapper, so the shell has to supply the font itself.
  env.EPAPER_FONT = consoleFont;

  shellHook = ''
    ${pre-commit-check.shellHook}

    export PS1="(waveshare) $PS1"
  '';
}
