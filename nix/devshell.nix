{
  inputs,
  pkgs,
  perSystem,
  ...
}:
let
  inherit (pkgs) lib;
  # TinyUSB and friends live in the SDK submodules; the plain derivation omits
  # them and USB support silently disappears from the CMake build.
  picoSdk = pkgs.pico-sdk.override { withSubmodules = true; };
  python = pkgs.python3.withPackages (ps: [
    ps.pyserial
    ps.pillow
    ps.pyte
  ]);
  pre-commit-check = import ./checks/pre-commit-check.nix { inherit inputs pkgs; };
  # serialport enumerates USB devices through libudev on Linux, a pkg-config
  # dependency of its default features. Darwin needs nothing declared: the
  # stdenv already supplies the SDK that IOKit enumeration links against.
  serialportInputs = lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.udev;
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

    pkgs.rustc
    pkgs.cargo
    pkgs.clippy
    pkgs.rustfmt
    pkgs.rust-analyzer
    pkgs.pkg-config

    pkgs.cmake
    pkgs.ninja
    pkgs.gcc-arm-embedded
    pkgs.picotool
    picoSdk
    python
    pkgs.mpremote # talks to the MicroPython REPL still on the board

    perSystem.self.epaper # the packaged Python CLI, alongside the source checkout
    # The Rust port, reachable as epaper-rs. It also ships bin/epaper, so it
    # comes after the Python package, which keeps the unsuffixed name until the
    # port is validated on hardware.
    perSystem.self.epaper-rs
  ]
  ++ serialportInputs;

  env.PICO_SDK_PATH = "${picoSdk}/lib/pico-sdk";

  # Running the checkout with `python -m epaper` bypasses the packaged
  # wrapper, so the shell has to supply the font itself.
  env.EPAPER_FONT = consoleFont;

  shellHook = ''
    ${pre-commit-check.shellHook}

    export PS1="(waveshare) $PS1"
  '';
}
