#!/bin/sh
# Applies the bundled uinput udev rule right away instead of after the next reboot. Never fails
# the installation: in containers or chroots udev may simply not be running.
if command -v udevadm >/dev/null 2>&1; then
    modprobe uinput >/dev/null 2>&1 || true
    udevadm control --reload-rules >/dev/null 2>&1 || true
    udevadm trigger --subsystem-match=misc --action=change >/dev/null 2>&1 || true
fi
exit 0
