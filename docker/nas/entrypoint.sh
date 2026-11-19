#!/bin/sh
set -eu
: "${NAS_USER:=alice}"
: "${NAS_PASSWORD:?set NAS_PASSWORD}"

adduser -D -H "$NAS_USER" 2>/dev/null || true
printf '%s\n%s\n' "$NAS_PASSWORD" "$NAS_PASSWORD" | smbpasswd -a -s "$NAS_USER" >/dev/null

mkdir -p /srv/projects /srv/public
echo "hello from the nas" > /srv/projects/hello.txt
echo "public file" > /srv/public/readme.txt
chown -R "$NAS_USER" /srv/projects
chmod -R a+rX /srv/public

exec smbd --foreground --no-process-group --debug-stdout
