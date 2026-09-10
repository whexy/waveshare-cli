{ pkgs, ... }:
pkgs.mkShell {
  packages = [
    pkgs.ffmpeg # AVFoundation frame grab from the OBS virtual camera
    pkgs.imagemagick # crop/deskew/threshold the captured panel
    pkgs.coreutils # `timeout --signal=KILL`; macOS ships no BSD equivalent
    pkgs.jq
  ];

  shellHook = ''
    export PS1="(waveshare) $PS1"
  '';
}
