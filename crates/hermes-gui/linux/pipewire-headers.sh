#!/usr/bin/env bash
# PipeWire headers for building the Linux client on Ubuntu 22.04 (PipeWire 0.3.48).
#
# The pipewire crate (0.10) needs newer headers than 22.04 has, but the client must still run
# where PipeWire is as old as 0.3.48. So it is built against Ubuntu 24.04's headers (1.0.5) and
# linked to the system's libpipewire-0.3.so.0, and `check` proves that the binary calls nothing
# 0.3.48 lacks (a missing symbol would stop it from starting there).
#
#   pipewire-headers.sh [dir]              downloads the headers; prints the PKG_CONFIG_PATH
#   pipewire-headers.sh check <binary>     fails if <binary> needs a PipeWire symbol this
#                                          system's libpipewire does not have
#
# Needs curl, dpkg-deb, and libpipewire-0.3-dev (22.04's, for the library itself).
set -eu

LIB=/usr/lib/x86_64-linux-gnu/libpipewire-0.3.so.0

if [ "${1:-}" = check ]; then
    binary="$2"
    needed=$(nm -D --undefined-only "$binary" | awk '{print $2}' | grep -E '^pw_' | sort -u || true)
    missing=$(comm -23 <(printf '%s\n' "$needed") \
        <(nm -D --defined-only "$LIB" | awk '{print $3}' | sort -u))
    if [ -n "$missing" ]; then
        echo "needs PipeWire functions $LIB lacks:" >&2
        printf '  %s\n' $missing >&2
        exit 1
    fi
    echo "PipeWire: $(printf '%s\n' "$needed" | grep -c .) functions used, all in $LIB"
    exit 0
fi

VERSION=1.0.5-1ubuntu3.3
DIR="${1:-/opt/pipewire-headers}"
POOL=https://archive.ubuntu.com/ubuntu/pool/main/p/pipewire
mkdir -p "$DIR"
cd "$DIR"
for pkg in libpipewire-0.3-dev libspa-0.2-dev; do
    deb="${pkg}_${VERSION}_amd64.deb"
    [ -f "$deb" ] || curl -fsSLO "$POOL/$deb"
    dpkg-deb -x "$deb" root
done
# The headers from here, the library from the system.
mkdir -p pkgconfig
for pc in libpipewire-0.3 libspa-0.2; do
    sed -e "s|^prefix=.*|prefix=$DIR/root/usr|" \
        -e "s|^libdir=.*|libdir=/usr/lib/x86_64-linux-gnu|" \
        "root/usr/lib/x86_64-linux-gnu/pkgconfig/$pc.pc" > "pkgconfig/$pc.pc"
done
echo "$DIR/pkgconfig"
