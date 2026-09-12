FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += " \
    file://main.conf \
    file://iwd-readonly-rootfs.conf \
    file://80-wlan.network \
"

do_install:append() {
    install -d ${D}${sysconfdir}/iwd
    install -d ${D}${sysconfdir}/systemd/system/iwd.service.d
    install -d ${D}${sysconfdir}/systemd/network

    install -m 0644 ${WORKDIR}/main.conf ${D}${sysconfdir}/iwd/main.conf
    install -m 0644 ${WORKDIR}/iwd-readonly-rootfs.conf \
        ${D}${sysconfdir}/systemd/system/iwd.service.d/readonly-rootfs.conf
    install -m 0644 ${WORKDIR}/80-wlan.network \
        ${D}${sysconfdir}/systemd/network/80-wlan.network
}

FILES:${PN} += " \
    ${sysconfdir}/iwd/main.conf \
    ${sysconfdir}/systemd/system/iwd.service.d/readonly-rootfs.conf \
    ${sysconfdir}/systemd/network/80-wlan.network \
"