#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
yocto_build_dir=${YOCTO_BUILD_DIR:-"$project_dir/../../kas/build"}
target_triple=aarch64-unknown-linux-gnu
tool_base="$yocto_build_dir/tmp/sysroots-components/x86_64"
gccroot="$tool_base/gcc-cross-aarch64/usr/bin/aarch64-poky-linux"
binutilsroot="$tool_base/binutils-cross-aarch64/usr/bin/aarch64-poky-linux"

mapfile -t sysroot_matches < <(compgen -G "$yocto_build_dir/tmp/work/cortexa72-poky-linux/weston/*/recipe-sysroot" || true)
if [[ ${#sysroot_matches[@]} -eq 0 ]]; then
    echo "error: no Yocto Weston recipe sysroot found under $yocto_build_dir" >&2
    exit 1
fi

sysroot=${sysroot_matches[0]}
sdl2_image_matches=( )
mapfile -t sdl2_image_matches < <(compgen -G "$yocto_build_dir/tmp/work/cortexa72-poky-linux/libsdl2/*/image" || true)
if [[ ${#sdl2_image_matches[@]} -eq 0 ]]; then
    echo "error: no Yocto SDL2 image found under $yocto_build_dir" >&2
    exit 1
fi

sdl2_image=${sdl2_image_matches[0]}
gcclibexec=$(compgen -G "$tool_base/gcc-cross-aarch64/usr/libexec/aarch64-poky-linux/gcc/aarch64-poky-linux/*" | head -1 || true)

if [[ -z "$gcclibexec" ]]; then
    echo "error: no Yocto GCC libexec directory found under $tool_base" >&2
    exit 1
fi

if [[ ! -x "$gccroot/aarch64-poky-linux-gcc" ]]; then
    echo "error: missing Yocto cross-compiler at $gccroot/aarch64-poky-linux-gcc" >&2
    exit 1
fi

if ! rustup target list --installed | grep -qx "$target_triple"; then
    rustup target add "$target_triple"
fi

export PATH="$binutilsroot:$gccroot:$gcclibexec:$PATH"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$gccroot/aarch64-poky-linux-gcc"
export CC_aarch64_unknown_linux_gnu="$gccroot/aarch64-poky-linux-gcc"
export CXX_aarch64_unknown_linux_gnu="$gccroot/aarch64-poky-linux-g++"
export PKG_CONFIG_ALLOW_CROSS=1
export SDL2_INCLUDE_PATH="$sdl2_image/usr/include"
export PKG_CONFIG_SYSROOT_DIR="$sysroot"
export PKG_CONFIG_PATH="$sysroot/usr/lib/pkgconfig:$sysroot/usr/share/pkgconfig:$sdl2_image/usr/lib/pkgconfig"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C link-arg=--sysroot=$sysroot -L native=$sdl2_image/usr/lib"

cargo build --release --target "$target_triple" \
    --features sdl-input \
    --bin openconsole \
    "$@"

cargo build --release --target "$target_triple" \
    --features sdl-input \
    --bin openconsole-ui \
    --bin openconsole-sdl-probe \
    "$@"

echo
echo "Built target/$target_triple/release/openconsole"
echo "Built target/$target_triple/release/openconsole-ui"
echo "Built target/$target_triple/release/openconsole-sdl-probe"
