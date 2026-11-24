#!/bin/bash
# Runs the installed Flatpak (.github/workflows/flatpak.yml): its tabs see
# the computer's own system folders, not the runtime's.
set -euo pipefail
id=org.patrone.FileManager
run() { dbus-run-session -- flatpak run "$id" "$@"; }
# The computer's /usr/share (Ubuntu's: dpkg), which the runtime's lacks
share=$(run --list /usr/share)
echo "$(wc -l <<< "$share") names in /usr/share"
grep -qx dpkg <<< "$share"
[[ $(flatpak run --command=ls "$id" /usr/share | grep -cx dpkg) -eq 0 ]]
# The account's files, at their own paths
mkdir -p "$HOME/fm-test" && touch "$HOME/fm-test/hello.txt"
run --list "$HOME/fm-test" | grep -qx hello.txt
flatpak run --command=ls "$id" /app/share/doc/patrone-file-manager/third-party-licenses.txt.gz \
    /app/share/metainfo/org.patrone.FileManager.metainfo.xml
echo "The Flatpak reads the computer's folders."
