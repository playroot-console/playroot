# Getting Started

## Prerequisites

You need at least the following:

- Raspberry Pi 4
- HDMI cable and power supply
- microSD card, 16 GB or larger recommended
- USB keyboard for first-time setup

## Install a Prebuilt Image

1. Download Raspberry Pi Imager: https://www.raspberrypi.com/software/
2. Download the PlayRoot prebuilt Raspberry Pi image.
3. Unpack the downloaded `.wic.bz2` file to get the `.wic` image.
4. Open Raspberry Pi Imager.
5. Select the Raspberry Pi 4 model. The current image target is Raspberry Pi 4.
6. For Operating System, choose `Use custom`.
7. Select the `.wic` file. If it does not appear, switch the file chooser to `All Files (*)`.
8. Select the destination storage device.
9. Press Next. When asked `Use OS customization`, choose `No`.
10. Confirm that the selected storage will be erased.
11. After writing completes, insert the card into the Raspberry Pi and boot the device.

## First Boot

- Keep a regular USB keyboard connected during the first boot. It is required for initial controller setup and can also be used for navigation and gameplay.
- Wi-Fi and Bluetooth are disabled by default. Enable Bluetooth in System Settings before pairing a Bluetooth controller.
- The launcher starts in full-screen console mode after boot.

## Internet

- The current game examples are baked into the build, so no internet connection is required for basic use.
- If you do enable network access, be aware that SSH access may also be available depending on your image configuration. Review the `Default Login Credentials` section before connecting the device to a shared or untrusted network.

## Default Login Credentials

The current image recipe creates both `root` and `pi` accounts with the default password `playroot`.

- `root` password: `playroot`
- `pi` password: `playroot`

If SSH is enabled for your device or network, change these credentials before using the image outside a controlled development environment.

## SSH Notes

- The image includes OpenSSH support.
- For public or shared deployments, do not keep the default password.
- Prefer replacing password login with SSH keys and removing or locking the development account for production images.
