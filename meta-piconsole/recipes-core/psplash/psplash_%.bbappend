FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

SPLASH_IMAGES:rpi:forcevariable = "file://psplash-openconsole-placeholder.png;outsuffix=default"
SRC_URI:append:rpi = " file://framebuf-openconsole.conf"

do_openconsole_fix_psplash_framebuf() {
	install -Dm 0644 ${WORKDIR}/framebuf-openconsole.conf ${D}${systemd_system_unitdir}/psplash-start.service.d/framebuf.conf
}

addtask openconsole_fix_psplash_framebuf after do_install before do_package