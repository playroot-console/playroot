SUMMARY = "Base runtime package group for Pi Console"
LICENSE = "MIT"

PACKAGE_ARCH = "${MACHINE_ARCH}"

inherit packagegroup

RDEPENDS:${PN} = " \
    alsa-utils \
    bluez5 \
    ca-certificates \
    curl \
    iwd \
    kmscube \
    libdecor \
    libevdev \
    libinput \
    libsdl2 \
    openssh \
    openssh-sftp-server \
    openconsole-runtime \
    evtest \
    pipewire \
    pipewire-alsa \
    rauc \
    uhubctl \
    weston \
    weston-init \
    wireplumber \
    wpa-supplicant \
"
