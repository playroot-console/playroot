CMDLINE_SERIAL = ""
CMDLINE_QUIET = "console=tty3 quiet splash loglevel=0 vt.global_cursor_default=0 systemd.show_status=false rd.udev.log_level=3 udev.log_level=3"

CMDLINE:append = " ${CMDLINE_QUIET}"