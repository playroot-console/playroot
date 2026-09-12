#!/usr/bin/env bash
set -euo pipefail

# Build server host from ~/.ssh/config
BUILD_HOST="${BUILD_HOST:-yt_server}"
# Optional password for BUILD_HOST authentication. Requires sshpass when set.
BUILD_HOST_PASS="${BUILD_HOST_PASS:-}"

# Build project path on build server
BUILD_PROJECT_DIR="${BUILD_PROJECT_DIR:-/home/m/yocto-openconsole/slintui/openconsole}"
TARGET_TRIPLE="${TARGET_TRIPLE:-aarch64-unknown-linux-gnu}"

# Raspberry Pi SSH target
PI_HOST="${PI_HOST:-192.168.178.151}"
PI_USER="${PI_USER:-pi}"
# Optional password for both PI_USER and root authentication. Requires sshpass when set.
CONSOLE_PASS="${CONSOLE_PASS:-}"

# Destination path on Pi
PI_DEPLOY_DIR="${PI_DEPLOY_DIR:-/data/games/project/openconsole}"
PI_GAME_DEPLOY_DIR="${PI_GAME_DEPLOY_DIR:-/data/games/project}"
NO_REBOOT="${NO_REBOOT:-0}"
RESTART_SERVICE="${RESTART_SERVICE:-1}"

# Temporary files
LOCAL_TARBALL="${LOCAL_TARBALL:-/tmp/openconsole-deploy.tgz}"
REMOTE_TARBALL="${REMOTE_TARBALL:-/tmp/openconsole-deploy.tgz}"
LOCAL_STAGE_DIR="$(mktemp -d /tmp/openconsole-deploy-local.XXXXXX)"
LOCAL_RUN_GAME_SCRIPT="${LOCAL_RUN_GAME_SCRIPT:-/home/m/yocto-openconsole/meta-piconsole/recipes-openconsole/openconsole-runtime/openconsole-runtime/run-game-as-weston.sh}"

KNOWN_HOSTS_FILE="${HOME}/.ssh/known_hosts"

if command -v sha256sum >/dev/null 2>&1; then
	LOCAL_SHA256_TOOL="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
	LOCAL_SHA256_TOOL="shasum"
else
	echo "Neither sha256sum nor shasum is available locally. Install coreutils or use macOS shasum." >&2
	exit 1
fi

write_local_sha256sums() {
	local stage_dir="$1"
	if [[ "${LOCAL_SHA256_TOOL}" == "sha256sum" ]]; then
		(
			cd "${stage_dir}"
			find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
		)
	else
		(
			cd "${stage_dir}"
			find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 shasum -a 256 > SHA256SUMS
		)
	fi
}


if [[ -n "${BUILD_HOST_PASS}" || -n "${CONSOLE_PASS}" ]] && ! command -v sshpass >/dev/null 2>&1; then
	echo "BUILD_HOST_PASS and CONSOLE_PASS require sshpass. Install it with: brew install sshpass" >&2
	exit 1
fi

build_ssh() {
	if [[ -n "${BUILD_HOST_PASS}" ]]; then
		SSHPASS="${BUILD_HOST_PASS}" sshpass -e ssh "$@"
	else
		ssh "$@"
	fi
}

build_scp() {
	if [[ -n "${BUILD_HOST_PASS}" ]]; then
		SSHPASS="${BUILD_HOST_PASS}" sshpass -e scp "$@"
	else
		scp "$@"
	fi
}

console_ssh() {
	if [[ -n "${CONSOLE_PASS}" ]]; then
		SSHPASS="${CONSOLE_PASS}" sshpass -e ssh "$@"
	else
		ssh "$@"
	fi
}

console_scp() {
	if [[ -n "${CONSOLE_PASS}" ]]; then
		SSHPASS="${CONSOLE_PASS}" sshpass -e scp "$@"
	else
		scp "$@"
	fi
}

if [[ -n "${CONSOLE_PASS}" ]]; then
	CONSOLE_PASS_SHELL=$(printf '%q' "${CONSOLE_PASS}")
else
	CONSOLE_PASS_SHELL=''
fi

if [[ -n "${BUILD_HOST_PASS}" ]]; then
	if ! command -v sshpass >/dev/null 2>&1; then
		echo "BUILD_HOST_PASS requires sshpass. Install it with: brew install sshpass" >&2
		exit 1
	fi
fi

cleanup() {
	rm -rf "${LOCAL_STAGE_DIR}"
}
trap cleanup EXIT

mkdir -p "${HOME}/.ssh"
touch "${KNOWN_HOSTS_FILE}"

echo "[1/8] Removing old SSH host keys for ${PI_HOST}"
ssh-keygen -R "${PI_HOST}" -f "${KNOWN_HOSTS_FILE}" >/dev/null 2>&1 || true
ssh-keygen -R "[${PI_HOST}]:22" -f "${KNOWN_HOSTS_FILE}" >/dev/null 2>&1 || true

echo "[2/8] Adding current SSH host key for ${PI_HOST}"
ssh-keyscan -H "${PI_HOST}" >> "${KNOWN_HOSTS_FILE}" 2>/dev/null || true

echo "[3/8] Building artifact and checksum package on ${BUILD_HOST}"
build_ssh "${BUILD_HOST}" "bash -lc '
set -e
export PATH=\"\$HOME/.cargo/bin:\$PATH\"
cd \"${BUILD_PROJECT_DIR}\"
# Build ARM64 binaries for Raspberry Pi
./build-pi-release.sh
STAGE_DIR=\"/tmp/openconsole-deploy-stage.\$\$\"
rm -rf \"\$STAGE_DIR\"
mkdir -p \"\$STAGE_DIR\"
cp target/${TARGET_TRIPLE}/release/openconsole \"\$STAGE_DIR/openconsole\"
cp target/${TARGET_TRIPLE}/release/openconsole-ui \"\$STAGE_DIR/openconsole-ui\"
if [ -f "target/${TARGET_TRIPLE}/release/openconsole-sdl-probe" ]; then
	cp target/${TARGET_TRIPLE}/release/openconsole-sdl-probe "\$STAGE_DIR/openconsole-sdl-probe"
fi
cp \"${BUILD_PROJECT_DIR}/games.json\" \"\$STAGE_DIR/games.json\"
cp -R "${BUILD_PROJECT_DIR}/assets" "\$STAGE_DIR/assets"
if [ -d "${BUILD_PROJECT_DIR}/games" ]; then
	cp -R "${BUILD_PROJECT_DIR}/games" "\$STAGE_DIR/games"
fi

RUN_GAME_SCRIPT_PATH=""
for candidate in \
	"/home/m/yocto-openconsole/meta-piconsole/recipes-openconsole/openconsole-runtime/openconsole-runtime/run-game-as-weston.sh" \
	"${BUILD_PROJECT_DIR}/run-game-as-weston.sh"; do
	if [ -f "\$candidate" ]; then
		RUN_GAME_SCRIPT_PATH="\$candidate"
		break
	fi
done

if [ -n "\$RUN_GAME_SCRIPT_PATH" ]; then
	echo "Using launcher script source: \$RUN_GAME_SCRIPT_PATH"
	cp "\$RUN_GAME_SCRIPT_PATH" "\$STAGE_DIR/run-game-as-weston.sh"
fi

SUPERVISOR_SERVICE_PATH=""
for candidate in \
	"${BUILD_PROJECT_DIR}/openconsole-supervisor.service" \
	"/home/m/yocto-openconsole/meta-piconsole/recipes-openconsole/openconsole-runtime/openconsole-runtime/openconsole-supervisor.service"; do
	if [ -f "\$candidate" ]; then
		SUPERVISOR_SERVICE_PATH="\$candidate"
		break
	fi
done

if [ -n "\$SUPERVISOR_SERVICE_PATH" ]; then
	cp "\$SUPERVISOR_SERVICE_PATH" "\$STAGE_DIR/openconsole-supervisor.service"
fi

(cd "\$STAGE_DIR" && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS)
tar -czf "${REMOTE_TARBALL}" -C "\$STAGE_DIR" .
rm -rf \"\$STAGE_DIR\"
'"

echo "[4/8] Downloading artifact from ${BUILD_HOST} to local machine"
build_scp "${BUILD_HOST}:${REMOTE_TARBALL}" "${LOCAL_TARBALL}"

echo "[5/8] Verifying local package contents and expected checksums"
tar -xzf "${LOCAL_TARBALL}" -C "${LOCAL_STAGE_DIR}"

if [ -f "${LOCAL_RUN_GAME_SCRIPT}" ]; then
	echo "Overriding packaged launcher with local source: ${LOCAL_RUN_GAME_SCRIPT}"
	cp "${LOCAL_RUN_GAME_SCRIPT}" "${LOCAL_STAGE_DIR}/run-game-as-weston.sh"
	chmod 755 "${LOCAL_STAGE_DIR}/run-game-as-weston.sh"
fi

if [ -f "${LOCAL_STAGE_DIR}/run-game-as-weston.sh" ]; then
	if ! grep -q "OPENCONSOLE_GAME_BIN" "${LOCAL_STAGE_DIR}/run-game-as-weston.sh"; then
		echo "ERROR: staged run-game-as-weston.sh is missing OPENCONSOLE_GAME_BIN support." >&2
		echo "Refusing to deploy stale launcher script." >&2
		exit 1
	fi
fi

write_local_sha256sums "${LOCAL_STAGE_DIR}"
tar -czf "${LOCAL_TARBALL}" -C "${LOCAL_STAGE_DIR}" .

echo "Expected hashes from build package:"
cat "${LOCAL_STAGE_DIR}/SHA256SUMS"

echo "[6/8] Uploading artifact to ${PI_USER}@${PI_HOST}"
console_scp "${LOCAL_TARBALL}" "${PI_USER}@${PI_HOST}:${REMOTE_TARBALL}"

echo "[7/8] Deploying on Pi as root and validating installed hashes"
console_ssh -tt "${PI_USER}@${PI_HOST}" "set -e
rm -rf /tmp/openconsole-deploy
mkdir -p /tmp/openconsole-deploy
tar -xzf '${REMOTE_TARBALL}' -C /tmp/openconsole-deploy

${CONSOLE_PASS_SHELL:+printf '%s\\n' ${CONSOLE_PASS_SHELL} | }su - -c '
set -e
mkdir -p \"${PI_DEPLOY_DIR}\"
mkdir -p "${PI_GAME_DEPLOY_DIR}"
cp -a /tmp/openconsole-deploy/. "${PI_DEPLOY_DIR}/"
chmod 755 \"${PI_DEPLOY_DIR}/openconsole\" \"${PI_DEPLOY_DIR}/openconsole-ui\"
chmod 644 \"${PI_DEPLOY_DIR}/games.json\" \"${PI_DEPLOY_DIR}/SHA256SUMS\"
if [ -f \"${PI_DEPLOY_DIR}/run-game-as-weston.sh\" ]; then
	chmod 755 \"${PI_DEPLOY_DIR}/run-game-as-weston.sh\"
fi
if [ -f \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\" ]; then
	chmod 644 \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\"
fi
if [ -d "${PI_DEPLOY_DIR}/assets" ]; then
	find "${PI_DEPLOY_DIR}/assets" -type d -exec chmod 755 {} +
	find "${PI_DEPLOY_DIR}/assets" -type f -exec chmod 644 {} +
	chown -R root:root "${PI_DEPLOY_DIR}/assets"
fi
if [ -d "${PI_DEPLOY_DIR}/games" ]; then
	find "${PI_DEPLOY_DIR}/games" -type d -exec chmod 755 {} +
	find "${PI_DEPLOY_DIR}/games" -type f -name "*.arm64" -exec chmod 755 {} +
	find "${PI_DEPLOY_DIR}/games" -type f ! -name "*.arm64" -exec chmod 644 {} +
	chown -R root:root "${PI_DEPLOY_DIR}/games"
	cp -a "${PI_DEPLOY_DIR}/games/." "${PI_GAME_DEPLOY_DIR}/"
	find "${PI_GAME_DEPLOY_DIR}" -maxdepth 1 -type f -name "*.arm64" -exec chmod 755 {} +
	find "${PI_GAME_DEPLOY_DIR}" -maxdepth 1 -type f ! -name "*.arm64" -exec chmod 644 {} +
	find "${PI_GAME_DEPLOY_DIR}" -maxdepth 1 -type f -exec chown root:root {} +
fi
find "${PI_DEPLOY_DIR}" -maxdepth 1 -type f -exec chown root:root {} +
cp \"${PI_DEPLOY_DIR}/SHA256SUMS\" \"${PI_DEPLOY_DIR}/.last-deploy-SHA256SUMS\"

echo \"Hash validation on Pi:\"
(cd \"${PI_DEPLOY_DIR}\" && sha256sum -c SHA256SUMS)

if [ -f \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\" ]; then
	if cmp -s \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\" /etc/systemd/system/openconsole-supervisor.service; then
		echo \"Supervisor service already matches the deployed copy.\"
	elif cp \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\" /etc/systemd/system/openconsole-supervisor.service; then
		chmod 644 /etc/systemd/system/openconsole-supervisor.service
		chown root:root /etc/systemd/system/openconsole-supervisor.service
		systemctl daemon-reload
	else
		echo \"WARNING: /etc is read-only; the deployed supervisor service was not activated. Rebuild the Yocto image to update it.\" >&2
	fi
fi

echo \"Installed file metadata:\"
stat -c \"%n size=%s mtime=%y ctime=%z\" \"${PI_DEPLOY_DIR}/openconsole\" \"${PI_DEPLOY_DIR}/openconsole-ui\" \"${PI_DEPLOY_DIR}/games.json\"
if [ -f \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\" ]; then
	stat -c \"%n size=%s mtime=%y ctime=%z\" \"${PI_DEPLOY_DIR}/openconsole-supervisor.service\"
	if [ -e /etc/systemd/system/openconsole-supervisor.service ]; then
		stat -c \"%n size=%s mtime=%y ctime=%z\" /etc/systemd/system/openconsole-supervisor.service
	else
		echo \"Active supervisor unit file:\"
		systemctl show openconsole-supervisor.service --property=FragmentPath --no-pager
	fi
fi
 


echo "Service ExecStart path:"
systemctl cat openconsole-supervisor.service | grep -E "^ExecStart=" || true
echo "Resolved systemd ExecStart:"
systemctl show openconsole-supervisor.service --property=ExecStart --property=MainPID --no-pager

if [ "${RESTART_SERVICE}" = "1" ]; then
	echo "Restarting openconsole-supervisor.service"
	echo "Sending SIGKILL to the entire service cgroup."
	systemctl kill --kill-who=all --signal=SIGKILL openconsole-supervisor.service || true
	systemctl start openconsole-supervisor.service
	systemctl is-active openconsole-supervisor.service
fi

rm -rf /tmp/openconsole-deploy "${REMOTE_TARBALL}"
EOF
'
"

echo "[8/8] Cleaning build-server temporary package"
build_ssh "${BUILD_HOST}" "rm -f '${REMOTE_TARBALL}'" || true

if [ "${NO_REBOOT}" = "1" ]; then
	echo "Done. Deployment verified. Reboot skipped because NO_REBOOT=1."
else
	echo "Requesting Pi reboot..."
	console_ssh -tt "${PI_USER}@${PI_HOST}" "${CONSOLE_PASS_SHELL:+printf '%s\\n' ${CONSOLE_PASS_SHELL} | }su - -c '/sbin/reboot'"
	echo "Done. Pi reboot was requested and SSH will disconnect."
fi
