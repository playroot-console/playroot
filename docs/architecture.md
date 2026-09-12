# Architecture

## Platform

The platform targets Raspberry Pi 4 in 64-bit mode on Yocto Scarthgap LTS. The graphics path uses the modern DRM/KMS stack and avoids the legacy Broadcom driver.

Boot flow:

1. Raspberry Pi firmware
2. U-Boot
3. Linux kernel with VC4/V3D DRM
4. systemd
5. Weston
6. Slint launcher
7. Godot game
8. Return to launcher

## A/B Strategy

The user-facing design describes two immutable OS slots and one persistent data partition. On Raspberry Pi, the practical implementation usually needs a boot filesystem and a root filesystem per slot so each slot stays self-contained.

Conceptually:

- Slot A: boot assets + read-only rootfs
- Slot B: boot assets + read-only rootfs
- Data: writable ext4 partition mounted at `/data`

Only the active and inactive OS slots participate in updates. The data partition is never rewritten by the update mechanism.

## Persistence Rules

Persistent state belongs under `/data`:

- `/data/games`
- `/data/downloads`
- `/data/configs`
- `/data/saves`
- `/data/screenshots`
- `/data/logs`
- `/data/cache`
- `/data/home`

The base root filesystem is expected to be mounted read-only.

## Graphics Stack

The graphics stack is ordered as follows:

1. Kernel DRM/KMS
2. VC4/V3D
3. Mesa
4. EGL
5. OpenGL ES
6. Wayland
7. Weston
8. Slint
9. Godot

Because the previous Buildroot attempt failed during EGL context creation, graphics validation is the first technical gate. Slint and Godot should not be added to the boot path until `kmscube`, Weston, and an EGL test all succeed on hardware.

## Audio

PipeWire with WirePlumber is the intended audio stack. ALSA remains the kernel-facing audio interface.

## Controllers

The base image should provide the input stack needed for:

- Generic HID (USB) controllers
- PlayStation 5 DualSense bluetooth controllers


The initial image includes SDL2, libinput, and evdev-related userspace packages. Final controller polish can follow after the display stack is stable.

## Updates

RAUC is the target update framework because it supports signed bundles, atomic installs, and rollback semantics that match the console requirements.

This scaffold includes RAUC in the base package set, but it does not yet define production slot metadata, bundle recipes, or bootloader integration details. Those should be added after the first graphics-validated boot image is running.
