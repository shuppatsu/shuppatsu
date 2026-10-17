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
          "x86_64-pc-windows-gnu"
        ];
      };

      mingw32 = pkgs.pkgsCross.mingw32;
      # The 64-bit Windows build of the game and tools (docs/building.md).
      mingwW64 = pkgs.pkgsCross.mingwW64;

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

      # The package: game and tools via nixpkgs' rustPlatform. The source is an explicit
      # allowlist, so no game data (data/, re/, out/, wine/, disc images) can reach the store.
      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./locales
          ./assets
          ./docs/settings.md
          ./LICENSE
        ];
      };

      mkDdg =
        {
          pname,
          extra ? { },
        }:
        pkgs.rustPlatform.buildRustPackage (
          {
            inherit pname src;
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "-p"
              pname
            ];
            cargoTestFlags = [
              "-p"
              pname
            ];
            nativeBuildInputs = [ pkgs.pkg-config ];
            # cpal links ALSA and gilrs links libudev at build time.
            buildInputs = with pkgs; [
              alsa-lib
              udev
            ];
            # Data tests skip without DDG_DATA_DIR / DDG_TRACE_DIR; never let the sandbox see one.
            preCheck = ''
              unset DDG_DATA_DIR DDG_TRACE_DIR
              export HOME=$TMPDIR XDG_CONFIG_HOME=$TMPDIR/config XDG_DATA_HOME=$TMPDIR/data
            '';
            # winit/wgpu/gilrs dlopen these. A RUNPATH rather than a wrapper script, so dlopen
            # from the binary finds them and current_exe() stays the real program (the Exec= of
            # `ddg-game install-desktop`). Added after fixupPhase's --shrink-rpath, which would
            # drop them as unused.
            postFixup = ''
              for f in $out/bin/*; do
                patchelf --add-rpath ${pkgs.lib.makeLibraryPath runtimeLibs} "$f"
              done
            '';
            postInstall = ''
              install -Dm644 LICENSE -t $out/share/doc/${pname}
              install -Dm644 assets/fonts/OFL-Mplus.txt -t $out/share/doc/${pname}
            '';
            meta = {
              description = "Native reimplementation of Densha de GO! 2 Kousoku-hen 3000-bandai (bring your own disc)";
              license = pkgs.lib.licenses.gpl3Plus;
              platforms = pkgs.lib.platforms.linux;
              mainProgram = pname;
            };
          }
          // extra
        );

      ddg-game = mkDdg {
        pname = "ddg-game";
        extra = {
          # The game's check runs every crate's tests (data tests skip in the sandbox).
          cargoTestFlags = [ "--workspace" ];
          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.copyDesktopItems
          ];
          desktopItems = [
            (pkgs.makeDesktopItem {
              name = "ddg-game";
              desktopName = "Densha de GO! 2 Kōsoku-hen 3000-bandai";
              comment = "Native reimplementation; plays from your own disc";
              exec = "ddg-game";
              # The icon comes from the user's own exe: `ddg-game install-desktop` writes it per
              # user under this name. The package ships none.
              icon = "ddg-game";
              categories = [
                "Game"
                "Simulation"
              ];
              startupWMClass = "ddg-game";
            })
          ];
        };
      };
      ddg-tools = mkDdg { pname = "ddg-tools"; };
    in
    {
      packages.${system} = {
        default = ddg-game;
        inherit ddg-game ddg-tools;
      };

      apps.${system} = {
        default = {
          type = "app";
          program = "${ddg-game}/bin/ddg-game";
        };
        ddg-tools = {
          type = "app";
          program = "${ddg-tools}/bin/ddg-tools";
        };
      };

      devShells.${system}.default = pkgs.mkShell {
        packages =
          [
            rust
            mingw32.stdenv.cc
            mingwW64.stdenv.cc
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
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = "${mingwW64.stdenv.cc}/bin/x86_64-w64-mingw32-gcc";
        # Static mcfgthread (as for i686), so the exe needs no MinGW DLLs next to it.
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L native=${mingwW64.windows.pthreads}/lib -L native=${mingwW64.windows.mcfgthreads}/lib -C link-arg=-l:libmcfgthread.a -C link-arg=-lkernel32 -C link-arg=-lntdll";
        GHIDRA_HOME = "${pkgs.ghidra}/lib/ghidra";
        # The original game needs a Japanese locale (ANSI code page 932) under Wine; the host
        # system may only ship en_US, so use the full glibc locale set.
        # nixpkgs glibc prefers the versioned variable, which NixOS sets to the system archive.
        LOCALE_ARCHIVE = "${pkgs.glibcLocales}/lib/locale/locale-archive";
        LOCALE_ARCHIVE_2_27 = "${pkgs.glibcLocales}/lib/locale/locale-archive";

        shellHook = ''
          # Default only when the extracted data exists, so data tests skip cleanly elsewhere.
          if [ -z "''${DDG_DATA_DIR:-}" ] && [ -d "$PWD/data/extracted" ]; then
            export DDG_DATA_DIR="$PWD/data/extracted"
          fi
          export WINEPREFIX="''${WINEPREFIX:-$PWD/wine/prefix}"
          # `cargo test/run --target x86_64-pc-windows-gnu` under Wine in its own prefix.
          export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER="$PWD/scripts/wine-run.sh"
        '';
      };
    };
}
