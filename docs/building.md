# Building the full image

For a true fresh build, remove the entire Yocto build output directory and build again:

```sh
cd kas
rm -rf build
kas shell scarthgap-rpi4.yml -c 'bitbake piconsole-image'
```

## Rebuild the image with the current caches

```sh
kas shell scarthgap-rpi4.yml -c 'bitbake -c cleansstate piconsole-image && bitbake piconsole-image'
```

This forces the image recipe itself to rebuild, but it can still reuse previously built dependencies from `kas/build/tmp` and `kas/build/sstate-cache`.

## Building and deploying only the UI

```sh
cd slintui/openconsole
./build-pi-release.sh
```

This builds only the UI. You can then copy it to the device over SSH and restart the launcher without rebuilding the full image or rewriting the SD card.

See the SSH section below for access details.

## Installing and testing a new Game

Games and the visible catalog are currently still baked into the project. This should change in the future. For now there is no UI flow for downloading a game, but you can use SSH to copy game files over for quick testing.

See the SSH section below.

After connecting over SSH, go to `/data/games/project/` to inspect the installed game files.

The UI presents games based on the JSON manifest. In the repository, see `slintui/openconsole/games.json` for the current game records.

Current games are created with Godot and will be made open-source soon. 
Godot settings rendering method is gl_compatibility or mobile. Export the game under linux, architecture setting arm64.

## SSH connection

Connect a network cable or enable Wi-Fi in Settings, then look up the device IP address and log in with the default `pi` user. For example:

```sh
ssh -X pi@192.168.178.100
```

Replace `192.168.178.100` with the actual device IP address.

Use the default password: `playroot`

You can switch to `root` with:

```sh
su -
```

The default `root` password is also `playroot`.

Change these passwords after the initial connection.
