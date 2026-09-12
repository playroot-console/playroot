SUMMARY = "OpenConsole U-Boot splash bitmap"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

COMPATIBLE_MACHINE = "^rpi$"

SRC_URI = "file://openconsole-u-boot-splash.bmp"

INHIBIT_DEFAULT_DEPS = "1"

inherit deploy nopackages

do_deploy() {
    install -d "${DEPLOYDIR}"
    install -m 0644 "${WORKDIR}/openconsole-u-boot-splash.bmp" "${DEPLOYDIR}/openconsole-u-boot-splash.bmp"
}

addtask do_deploy after do_compile before do_build