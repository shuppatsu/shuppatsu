{
  description = "Densha de GO! 2 Kousoku-hen 3000-bandai — native Rust reimplementation";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };

      rust = pkgs.rust-bin.stable.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
          "clippy"
          "rustfmt"
        ];
        targets = [
          "x86_64-unknown-linux-gnu"
          "i686-pc-windows-gnu"
        ];
      };

      mingw32 = pkgs.pkgsCross.mingw32;

      # Libraries winit/wgpu dlopen at runtime.
      runtimeLibs = with pkgs; [
        vulkan-loader
        wayland
        libxkbcommon
        libx11
        libxcursor
        libxi
        libxrandr
        alsa-lib
        udev
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages =
          [
            rust
            mingw32.stdenv.cc
          ]
          ++ (with pkgs; [
            pkg-config
            vulkan-tools
            vulkan-validation-layers

            # disc extraction and inspection
            bchunk
            p7zip
            libarchive
            unshield
            hexyl
            binwalk
            binutils

            # reverse engineering / oracle
            ghidra
            # 11.0 stable segfaults in glibc 2.44 __tunables_init via wine-preloader; 11.19 works.
            wineWow64Packages.unstable
          ])
          ++ runtimeLibs;

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
        CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER = "${mingw32.stdenv.cc}/bin/i686-w64-mingw32-gcc";
        # nixpkgs mingw gcc uses the mcfgthread threading model; libgcc_eh needs it.
        CARGO_TARGET_I686_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${mingw32.windows.pthreads}/lib -L native=${mingw32.windows.mcfgthreads}/lib -C link-arg=-l:libmcfgthread.a -C link-arg=-lkernel32 -C link-arg=-lntdll";
        GHIDRA_HOME = "${pkgs.ghidra}/lib/ghidra";
        # The original game needs a Japanese locale (ANSI code page 932) under Wine; the host
        # system may only ship en_US, so use the full glibc locale set.
        LOCALE_ARCHIVE = "${pkgs.glibcLocales}/lib/locale/locale-archive";

        shellHook = ''
          export DDG_DATA_DIR="''${DDG_DATA_DIR:-$PWD/data/extracted}"
          export WINEPREFIX="''${WINEPREFIX:-$PWD/wine/prefix}"
        '';
      };
    };
}
