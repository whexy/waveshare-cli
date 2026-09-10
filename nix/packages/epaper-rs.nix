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
  fontPackage = pkgs.nerd-fonts.terminess-ttf;
  fontFile =
    "${fontPackage}/share/fonts/truetype/NerdFonts/Terminess/" + "TerminessNerdFontMono-Regular.ttf";
in
pkgs.rustPlatform.buildRustPackage {
  inherit pname;
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../../cli-rs;
    fileset = lib.fileset.unions [
      ../../cli-rs/Cargo.toml
      ../../cli-rs/Cargo.lock
      ../../cli-rs/src
    ];
  };

  cargoLock.lockFile = ../../cli-rs/Cargo.lock;

  nativeBuildInputs = [
    pkgs.makeWrapper
  ]
  ++ lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.pkg-config;

  # serialport enumerates USB devices through libudev on Linux. Darwin needs
  # nothing declared: the stdenv already supplies the SDK that IOKit links
  # against.
  buildInputs = lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.udev;

  # The integration test drives tools/epdsim.py, which is outside this source
  # root, and the font tests need a font; both are covered by `cargo test` in
  # the devshell instead.
  doCheck = false;

  # A default, not a hard override: an EPAPER_FONT already in the environment
  # wins, so users can supply their own patched font.
  #
  # Installed as epaper-rs as well so both CLIs can sit in one PATH: the Python
  # package owns the `epaper` name until the owner has validated this one on
  # real hardware.
  postFixup = ''
    wrapProgram $out/bin/epaper --set-default EPAPER_FONT ${fontFile}
    ln -s $out/bin/epaper $out/bin/epaper-rs
  '';

  meta = {
    description = "Host CLI for the Pico 2 driven Waveshare 7.5\" e-paper panel";
    homepage = "https://github.com/whexy/waveshare";
    mainProgram = "epaper";
    platforms = lib.platforms.unix;
  };
}
