SUMMARY = "Base Raspberry Pi console image"
DESCRIPTION = "Immutable-style base image for Raspberry Pi 4 bring-up with Weston, RAUC, and graphics validation tools"
LICENSE = "MIT"

inherit core-image extrausers

IMAGE_FEATURES += "read-only-rootfs ssh-server-openssh splash"
IMAGE_INSTALL:append = " packagegroup-piconsole-base"
SYSTEMD_DEFAULT_TARGET = "graphical.target"

IMAGE_FSTYPES = "wic.bz2"
WKS_FILE = "piconsole-ab.wks.in"

PICONSOLE_PI_PASSWORD = "\$6\$iQnBrXK1wXt9RJqQ\$EINGVHwLtyjv3Lc9dnClw6quTN2sUUUl2OA.rTWSr4re7COg2n4rHBERQ77jIM25uvz15Pb4DWNxNxW.F4D1X1"

EXTRA_USERS_PARAMS = " \
    usermod -p '${PICONSOLE_PI_PASSWORD}' root; \
    groupadd netdev; \
    groupadd pi; \
    useradd -p '${PICONSOLE_PI_PASSWORD}' -g pi -G audio,input,video,render,netdev,wayland -M -d /data/home/pi -s /bin/sh pi; \
"

IMAGE_ROOTFS_EXTRA_SPACE ?= "1048576"
