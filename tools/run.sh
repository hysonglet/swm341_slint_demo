#!/usr/bin/env bash
# cargo runner: one command for both memories.
#
# The ELF holds a `.sfc_xip` section whose addresses lie inside the SFC window
# (0x7000_0000) -- the SPI NOR -- holding both the pictures and the code that
# runs from there. probe-rs only flashes the internal-flash region by itself,
# so this cuts that section out and programs it with probe-rs' own
# `swm341_spiflash` algorithm, then flashes the firmware and attaches RTT.
#
# The SPI NOR is only touched when the section actually changed (its hash is
# kept in assets/build/), because programming ~1 MB over SWD takes a while.
set -euo pipefail
cd "$(dirname "$0")/.."

ELF=${1:?usage: run.sh <elf>}

hash_of() { (shasum -a 256 "$1" 2>/dev/null || sha256sum "$1") | cut -d' ' -f1; }

# objdump -h columns: Idx Name Size VMA LMA File-off Align
LMA=$(arm-none-eabi-objdump -h "$ELF" | awk '$2 == ".sfc_xip" {print $5}')
if [ -n "$LMA" ]; then
    mkdir -p assets/build
    arm-none-eabi-objcopy -O binary --only-section=.sfc_xip "$ELF" assets/build/sfc_xip.bin

    NEW=$(hash_of assets/build/sfc_xip.bin)
    OLD=$(cat assets/build/sfc_xip.sha 2>/dev/null || true)
    if [ "$NEW" != "$OLD" ]; then
        echo "=== SPI NOR: programming $(wc -c < assets/build/sfc_xip.bin | tr -d ' ') bytes at 0x$LMA"
        probe-rs download --chip swm341 --binary-format bin --base-address "0x$LMA" \
            assets/build/sfc_xip.bin
        printf '%s' "$NEW" > assets/build/sfc_xip.sha
    else
        echo "=== SPI NOR: already holds this image"
    fi
fi

echo "=== internal flash: $ELF"
exec probe-rs run --chip swm341 "$ELF"
