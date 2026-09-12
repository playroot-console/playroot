do_configure:append:rpi() {
    sed -i '/^CONFIG_BOOTDELAY=/d' "${B}/.config"
    sed -i '/^CONFIG_SILENT_CONSOLE=/d' "${B}/.config"
    sed -i '/^CONFIG_SILENT_U_BOOT_ONLY=/d' "${B}/.config"
    sed -i '/^CONFIG_SYS_DEVICE_NULLDEV=/d' "${B}/.config"
    sed -i '/^CONFIG_VIDEO_LOGO=/d' "${B}/.config"
    sed -i '/^CONFIG_CMD_BMP=/d' "${B}/.config"
    sed -i '/^CONFIG_SPLASH_SCREEN=/d' "${B}/.config"
    sed -i '/^CONFIG_SPLASH_SCREEN_ALIGN=/d' "${B}/.config"
    sed -i '/^CONFIG_HIDE_LOGO_VERSION=/d' "${B}/.config"
    sed -i '/^CONFIG_BMP_24BPP=/d' "${B}/.config"

    env_file="${S}/board/raspberrypi/rpi/rpi.env"
    if [ -f "${env_file}" ]; then
        sed -i '/^silent=/d' "${env_file}"
        printf '\nsilent=1\n' >>"${env_file}"
    else
        bbfatal "Expected Raspberry Pi U-Boot environment file at ${env_file}"
    fi

    cat >>"${B}/.config" <<'EOF'
CONFIG_BOOTDELAY=0
CONFIG_SILENT_CONSOLE=y
CONFIG_SILENT_U_BOOT_ONLY=y
CONFIG_SYS_DEVICE_NULLDEV=y
CONFIG_CMD_BMP=y
CONFIG_SPLASH_SCREEN=y
CONFIG_SPLASH_SCREEN_ALIGN=y
CONFIG_HIDE_LOGO_VERSION=y
CONFIG_BMP_24BPP=y
# Keep U-Boot visually blank; the branded splash is provided later by psplash.
# CONFIG_VIDEO_LOGO is not set
EOF
}