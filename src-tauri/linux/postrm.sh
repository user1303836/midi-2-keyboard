#!/bin/sh
# Withdraws the uinput access granted by the removed udev rule (on upgrades the new version's
# post-install script grants it again). Never fails the removal.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules >/dev/null 2>&1 || true
    udevadm trigger --subsystem-match=misc --action=change >/dev/null 2>&1 || true
fi
exit 0
