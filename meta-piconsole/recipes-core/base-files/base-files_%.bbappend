FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SRC_URI += "file://piconsole.conf"

do_install:append() {
    install -d ${D}/data
    install -d ${D}${nonarch_libdir}/tmpfiles.d
    install -m 0644 ${WORKDIR}/piconsole.conf ${D}${nonarch_libdir}/tmpfiles.d/piconsole.conf
}
