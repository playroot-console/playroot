SUMMARY = "OpenConsole runtime payload and boot services"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

PACKAGE_ARCH = "${MACHINE_ARCH}"

inherit systemd

SRC_URI = " \
    file://openconsole-prepare.sh \
    file://openconsole-prepare.service \
    file://openconsole-supervisor.service \
    file://openconsole-wait-for-wayland.sh \
    file://run-game-as-weston.sh \
"

S = "${WORKDIR}"

OPENCONSOLE_WORKSPACE_DIR ?= "/home/m/yocto-openconsole/slintui/openconsole"
OPENCONSOLE_BUILD_DIR ?= "${OPENCONSOLE_WORKSPACE_DIR}/target/aarch64-unknown-linux-gnu/release"
OPENCONSOLE_GAMES_DIR ?= "${OPENCONSOLE_WORKSPACE_DIR}/games"

RDEPENDS:${PN} = "bash fontconfig libsdl2 shadow weston"

# These binaries are built outside BitBake and copied in as prebuilt payloads.
INSANE_SKIP:${PN} += "already-stripped ldflags"

SYSTEMD_SERVICE:${PN} = "openconsole-prepare.service openconsole-supervisor.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

do_install() {
    install -d ${D}${systemd_system_unitdir}
    install -d ${D}${sysconfdir}/systemd/system
    install -d ${D}${libexecdir}
    install -d ${D}${datadir}/openconsole-seed/openconsole
    install -d ${D}${datadir}/openconsole-seed/openconsole/assets
    install -d ${D}${datadir}/openconsole-seed/game

    install -m 0644 ${WORKDIR}/openconsole-prepare.service ${D}${systemd_system_unitdir}/openconsole-prepare.service
    install -m 0644 ${WORKDIR}/openconsole-supervisor.service ${D}${systemd_system_unitdir}/openconsole-supervisor.service
    install -m 0644 ${WORKDIR}/openconsole-supervisor.service ${D}${sysconfdir}/systemd/system/openconsole-supervisor.service
    install -m 0755 ${WORKDIR}/openconsole-prepare.sh ${D}${libexecdir}/openconsole-prepare.sh
    install -m 0755 ${WORKDIR}/openconsole-wait-for-wayland.sh ${D}${libexecdir}/openconsole-wait-for-wayland.sh
    install -m 0755 ${WORKDIR}/run-game-as-weston.sh ${D}${datadir}/openconsole-seed/openconsole/run-game-as-weston.sh
    install -m 0644 ${WORKDIR}/openconsole-supervisor.service ${D}${datadir}/openconsole-seed/openconsole/openconsole-supervisor.service

    if [ ! -x ${OPENCONSOLE_BUILD_DIR}/openconsole ]; then
        bbfatal "Missing supervisor binary at ${OPENCONSOLE_BUILD_DIR}/openconsole"
    fi
    if [ ! -x ${OPENCONSOLE_BUILD_DIR}/openconsole-ui ]; then
        bbfatal "Missing UI binary at ${OPENCONSOLE_BUILD_DIR}/openconsole-ui"
    fi
    if [ ! -x ${OPENCONSOLE_BUILD_DIR}/openconsole-sdl-probe ]; then
        bbfatal "Missing SDL probe binary at ${OPENCONSOLE_BUILD_DIR}/openconsole-sdl-probe"
    fi
    if [ ! -f ${OPENCONSOLE_WORKSPACE_DIR}/games.json ]; then
        bbfatal "Missing games.json at ${OPENCONSOLE_WORKSPACE_DIR}/games.json"
    fi
    if [ ! -d ${OPENCONSOLE_WORKSPACE_DIR}/assets ]; then
        bbfatal "Missing assets directory at ${OPENCONSOLE_WORKSPACE_DIR}/assets"
    fi
    if [ ! -d ${OPENCONSOLE_GAMES_DIR} ]; then
        bbfatal "Missing games directory at ${OPENCONSOLE_GAMES_DIR}"
    fi

    install -m 0755 ${OPENCONSOLE_BUILD_DIR}/openconsole ${D}${datadir}/openconsole-seed/openconsole/openconsole
    install -m 0755 ${OPENCONSOLE_BUILD_DIR}/openconsole-ui ${D}${datadir}/openconsole-seed/openconsole/openconsole-ui
    install -m 0755 ${OPENCONSOLE_BUILD_DIR}/openconsole-sdl-probe ${D}${datadir}/openconsole-seed/openconsole/openconsole-sdl-probe
    install -m 0644 ${OPENCONSOLE_WORKSPACE_DIR}/games.json ${D}${datadir}/openconsole-seed/openconsole/games.json
    cp -R --no-preserve=ownership ${OPENCONSOLE_WORKSPACE_DIR}/assets/. ${D}${datadir}/openconsole-seed/openconsole/assets/

    for game_file in ${OPENCONSOLE_GAMES_DIR}/*; do
        if [ ! -f "$game_file" ]; then
            continue
        fi

        case "$game_file" in
            *.arm64)
                install -m 0755 "$game_file" ${D}${datadir}/openconsole-seed/game/$(basename "$game_file")
                ;;
            *)
                install -m 0644 "$game_file" ${D}${datadir}/openconsole-seed/game/$(basename "$game_file")
                ;;
        esac
    done
}

FILES:${PN} += " \
    ${systemd_system_unitdir}/openconsole-prepare.service \
    ${systemd_system_unitdir}/openconsole-supervisor.service \
    ${sysconfdir}/systemd/system/openconsole-supervisor.service \
    ${libexecdir}/openconsole-prepare.sh \
    ${libexecdir}/openconsole-wait-for-wayland.sh \
    ${datadir}/openconsole-seed \
"