#!/bin/sh
# Container entrypoint for `mgmt web`. Idempotent: safe to restart.
#   1. ensure the XDG dirs exist inside the /data volume
#   2. seed web.public_origin into config.yaml (enables Secure cookies + CSRF check)
#   3. on first boot, set the web password from $MGMT_WEB_PASSWORD
#   4. exec the server on $MGMT_BIND
set -eu

: "${XDG_CONFIG_HOME:=/data/config}"
: "${XDG_DATA_HOME:=/data/state}"
export XDG_CONFIG_HOME XDG_DATA_HOME

CFG_DIR="$XDG_CONFIG_HOME/mgmt"
mkdir -p "$CFG_DIR" "$XDG_DATA_HOME/mgmt"

CFG="$CFG_DIR/config.yaml"
AUTH="$CFG_DIR/web-auth.yaml"

# public_origin has no CLI flag, so write a minimal web block if the user did not
# mount their own config with one already.
if [ -n "${MGMT_PUBLIC_ORIGIN:-}" ] && ! grep -qs 'public_origin' "$CFG"; then
    if grep -qs '^web:' "$CFG"; then
        echo "warning: config has a web: block but no public_origin; leaving it alone" >&2
    else
        printf 'web:\n  public_origin: "%s"\n' "$MGMT_PUBLIC_ORIGIN" >>"$CFG"
    fi
fi

# First boot only: derive the password hash from the env secret. `mgmt web setpass`
# reads $MGMT_WEB_PASSWORD itself.
if [ -n "${MGMT_WEB_PASSWORD:-}" ] && ! grep -qs 'password_hash' "$AUTH"; then
    echo "seeding web password from \$MGMT_WEB_PASSWORD" >&2
    mgmt web setpass
fi

exec mgmt web serve --bind "${MGMT_BIND:-0.0.0.0:8321}"
