#!/usr/bin/env bash
# flash-usb.sh — write oscortex.iso to a USB drive (M8: real hardware boot)
#
# Usage:
#   ./scripts/flash-usb.sh                  # auto-detect + interactive confirm
#   ./scripts/flash-usb.sh /dev/diskN       # explicit device (macOS)
#   ./scripts/flash-usb.sh /dev/sdX         # explicit device (Linux)
#
# The ISO is already a hybrid BIOS/UEFI image produced by build-iso.sh:
#   • El Torito (BIOS CD boot) via limine-bios-cd.bin
#   • EFI System Partition  (UEFI boot) via limine-uefi-cd.bin
#   • MBR protective label + limine bios-install stage-2
# So dd'ing the .iso to a USB drive boots on both legacy BIOS and UEFI firmware.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ISO="$ROOT/oscortex.iso"

# ── Helpers ───────────────────────────────────────────────────────────────────

red()    { printf '\033[0;31m%s\033[0m\n' "$*"; }
green()  { printf '\033[0;32m%s\033[0m\n' "$*"; }
yellow() { printf '\033[0;33m%s\033[0m\n' "$*"; }
bold()   { printf '\033[1m%s\033[0m\n' "$*"; }

die() { red "ERROR: $*"; exit 1; }

# ── Verify ISO exists ─────────────────────────────────────────────────────────

[[ -f "$ISO" ]] || die "ISO not found at $ISO — run scripts/build-iso.sh first"

ISO_SIZE=$(du -sh "$ISO" | cut -f1)
bold "OSCortex USB Flash Utility"
echo  "  ISO: $ISO ($ISO_SIZE)"
echo  "  Hybrid BIOS+UEFI — works on most x86_64 machines"
echo

# ── Detect or accept device ───────────────────────────────────────────────────

PLATFORM="$(uname -s)"

if [[ -n "${1:-}" ]]; then
    DEVICE="$1"
else
    if [[ "$PLATFORM" == "Darwin" ]]; then
        echo "Removable disks detected:"
        diskutil list | grep -E '/dev/disk[0-9]+' | head -20
        echo
        printf "Enter target disk (e.g. /dev/disk4, NOT a partition): "
        read -r DEVICE
    else
        echo "Block devices:"
        lsblk -d -o NAME,SIZE,VENDOR,MODEL | head -20
        echo
        printf "Enter target device (e.g. /dev/sdb, NOT a partition): "
        read -r DEVICE
    fi
fi

# ── Safety checks ─────────────────────────────────────────────────────────────

[[ -b "$DEVICE" ]] || die "$DEVICE is not a block device"

# Refuse to write to anything that looks like an internal OS disk.
if [[ "$PLATFORM" == "Darwin" ]]; then
    # disk0 is almost always the internal SSD on macOS
    [[ "$DEVICE" == "/dev/disk0" ]] && die "Refusing to write to disk0 (internal SSD)"
    # Verify it's an external disk
    INFO=$(diskutil info "$DEVICE" 2>/dev/null) || die "diskutil info failed for $DEVICE"
    if ! echo "$INFO" | grep -q "Removable Media:.*Yes\|Protocol:.*USB\|Protocol:.*Thunderbolt"; then
        yellow "WARNING: $DEVICE does not appear to be a removable/USB drive."
        printf "Continue anyway? [y/N] "
        read -r CONFIRM
        [[ "$CONFIRM" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 0; }
    fi
    DISK_SIZE=$(diskutil info "$DEVICE" | grep "Disk Size:" | awk '{print $3, $4}')
else
    # Linux: refuse nvme0 or sda if it looks like the boot drive
    [[ "$DEVICE" =~ nvme0n1|sda$ ]] && {
        yellow "WARNING: $DEVICE looks like it could be your boot drive."
        printf "Continue anyway? [y/N] "
        read -r CONFIRM
        [[ "$CONFIRM" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 0; }
    }
    DISK_SIZE=$(lsblk -dn -o SIZE "$DEVICE" 2>/dev/null || echo "unknown")
fi

# ── Final confirmation ────────────────────────────────────────────────────────

echo
yellow "!!! ALL DATA ON $DEVICE WILL BE DESTROYED !!!"
echo   "  Device : $DEVICE"
echo   "  Size   : ${DISK_SIZE:-unknown}"
echo   "  Image  : $ISO ($ISO_SIZE)"
echo
printf "Type YES to confirm: "
read -r CONFIRM
[[ "$CONFIRM" == "YES" ]] || { echo "Aborted."; exit 0; }

# ── Flash ─────────────────────────────────────────────────────────────────────

if [[ "$PLATFORM" == "Darwin" ]]; then
    # Unmount all partitions on the disk before writing
    diskutil unmountDisk "$DEVICE" || true
    RAW_DEVICE="${DEVICE/\/dev\/disk//dev/rdisk}"  # use raw device for speed
    echo
    echo "Flashing to $RAW_DEVICE..."
    sudo dd if="$ISO" of="$RAW_DEVICE" bs=1m status=progress 2>&1 || \
    sudo dd if="$ISO" of="$RAW_DEVICE" bs=1048576          # fallback (no status)
    sync
    diskutil eject "$DEVICE" || true
else
    echo
    echo "Flashing to $DEVICE..."
    sudo dd if="$ISO" of="$DEVICE" bs=4M status=progress oflag=sync
    sync
fi

echo
green "Done! Remove the USB drive and boot any x86_64 machine from it."
green "Supports: Legacy BIOS (CSM) and UEFI — select USB in your boot menu."
