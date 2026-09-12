FILESEXTRAPATHS:prepend := "${THISDIR}/weston-init:"

do_install:append() {
    install -D -m 0644 ${WORKDIR}/weston.ini ${D}${sysconfdir}/xdg/weston/weston.ini
}