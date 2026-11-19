#!/bin/sh
# Cargo's runner for macOS dev builds (.cargo/config.toml). Every rebuild gets a new throwaway
# signature, so macOS forgets "Always Allow" on the keychain and asks for the password again.
# Signing the app with a real certificate under a fixed name makes the grant stick.
# Uses $MARKON_SIGN_IDENTITY, else the first Apple Development certificate. With none, runs as before.
bin="$1"
if [ "$(basename "$bin")" = "markon-desktop" ] && codesign -dv "$bin" 2>&1 | grep -q "Signature=adhoc"; then
  id="${MARKON_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
  if [ -n "$id" ]; then
    codesign --force --timestamp=none -s "$id" -i com.markon.desktop.dev "$bin" ||
      echo "[dev-sign] signing failed: the keychain will ask for the password again" >&2
  fi
fi
exec "$@"
