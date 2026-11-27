#!/bin/sh
set -eu

if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'This installer requires macOS.' >&2
    exit 1
fi

project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
data="$HOME/Library/Application Support/org.Surveillance.Surveillance"
package="$data/startup-package"
agents="$HOME/Library/LaunchAgents"
label=org.surveillance.host.startup
domain="gui/$(id -u)"
plist="$agents/$label.plist"

umask 077
mkdir -p "$data" "$agents"
/bin/launchctl print "$domain" >/dev/null
nix build "$project" --out-link "$package"
app="$package/Applications/Surveillance.app"
/usr/bin/codesign --verify --deep --strict "$app"

pending=$(/usr/bin/mktemp "$agents/.$label.XXXXXX")
trap '/bin/rm "$pending"' EXIT
/usr/bin/plutil -create xml1 "$pending"
/usr/bin/plutil -insert Label -string "$label" "$pending"
/usr/bin/plutil -insert AssociatedBundleIdentifiers -array "$pending"
/usr/bin/plutil -insert AssociatedBundleIdentifiers.0 -string org.surveillance.host "$pending"
/usr/bin/plutil -insert RunAtLoad -bool YES "$pending"
/usr/bin/plutil -insert ProgramArguments -array "$pending"
/usr/bin/plutil -insert ProgramArguments.0 -string /bin/sh "$pending"
/usr/bin/plutil -insert ProgramArguments.1 -string -c "$pending"
/usr/bin/plutil -insert ProgramArguments.2 -string '
    if [ -d /Applications/Tailscale.app ] && ! /usr/bin/pgrep -x -u "$(/usr/bin/id -u)" Tailscale >/dev/null; then
        /usr/bin/open -g -j -a /Applications/Tailscale.app
    fi
    exec /usr/bin/open -g -j "$1"
' "$pending"
/usr/bin/plutil -insert ProgramArguments.3 -string surveillance-startup "$pending"
/usr/bin/plutil -insert ProgramArguments.4 -string "$app" "$pending"
/usr/bin/plutil -lint "$pending"

if /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
    /bin/launchctl bootout "$domain/$label"
fi
mv "$pending" "$plist"
trap - EXIT
/bin/launchctl enable "$domain/$label"
/bin/launchctl bootstrap "$domain" "$plist"
printf '%s\n' 'Surveillance is registered to start at macOS login.'
