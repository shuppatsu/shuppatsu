#!/usr/bin/env bash
# Present the user's own disc image (MDF/MDS, full TOC) to Wine as a real CD drive via CDEmu,
# so the original game's disc check runs unmodified. Run inside `nix develop`.
#
# One-time per boot, as root (no NixOS config change needed):
#   nix build --impure --expr 'with import <nixpkgs> {}; linuxPackages_latest.vhba' -o /tmp/vhba
#   sudo insmod /tmp/vhba/lib/modules/$(uname -r)/updates/vhba.ko.xz
#   sudo setfacl -m u:$USER:rw /dev/vhba_ctl
#
# Usage: scripts/mount-disc.sh [image.mds]   (default: data/disc/DENGO3K.mds)
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
image=$(realpath "${1:-data/disc/DENGO3K.mds}")
cdemu() { nix shell nixpkgs#cdemu-client -c cdemu "$@"; }

[[ -w /dev/vhba_ctl ]] || { echo "vhba not loaded or /dev/vhba_ctl not writable; see header" >&2; exit 1; }

if ! cdemu status >/dev/null 2>&1; then
  mkdir -p out
  nohup nix shell nixpkgs#cdemu-daemon -c cdemu-daemon --ctl-device=/dev/vhba_ctl \
    --num-devices=1 --bus=session --audio-driver=null > out/cdemu-daemon.log 2>&1 &
  for _ in $(seq 30); do cdemu status >/dev/null 2>&1 && break; sleep 1; done
fi

cdemu unload 0 >/dev/null 2>&1 || true
cdemu load 0 "$image"
sleep 2
dev=$(ls /dev/sr* | head -1)
findmnt -n "$dev" >/dev/null || udisksctl mount -b "$dev"
mnt=$(findmnt -n -o TARGET "$dev")

ln -sfn "$mnt" "$WINEPREFIX/dosdevices/d:"
ln -sfn "$dev" "$WINEPREFIX/dosdevices/d::"
wine reg add 'HKLM\Software\Wine\Drives' /v 'd:' /d cdrom /f >/dev/null
echo "disc $image -> $dev mounted at $mnt, Wine drive D:"
