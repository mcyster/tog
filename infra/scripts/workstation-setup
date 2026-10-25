#!/usr/bin/env bash
#
# First-boot host setup for a tog workstation: create the tog user and install
# the development environment (nix, direnv, nix-direnv, git) with a checkout of
# the tog repository at /home/tog/tog/main.
#
# Runs once on a fresh instance, injected as cloud-init user-data by
# tog-workstation create. Idempotent: re-running changes nothing it already did.
# Reuse the pattern elsewhere: change TOG_REPO_URL and TOG_CHECKOUT_DIR.

set -o errexit
set -o nounset
set -o pipefail

readonly TOG_USER="tog"
readonly TOG_HOME="/home/${TOG_USER}"
readonly TOG_REPO_URL="https://github.com/mcyster/tog.git"
readonly TOG_CHECKOUT_DIR="${TOG_HOME}/tog/main"
readonly SETUP_MARKER="/var/lib/tog-host-setup.done"

if [[ -f "$SETUP_MARKER" ]]; then
  exit 0
fi

export DEBIAN_FRONTEND=noninteractive

apt_get_install() {
  apt-get install --yes --no-install-recommends "$@" >/dev/null
}

install_apt_packages() {
  apt-get update >/dev/null
  apt_get_install curl git ca-certificates
}

create_tog_user() {
  if ! id "$TOG_USER" >/dev/null 2>&1; then
    useradd --create-home --shell /bin/bash "$TOG_USER"
  fi
  usermod -aG sudo "$TOG_USER"
}

install_nix() {
  if command -v nix >/dev/null 2>&1 && [[ -d /nix/store ]]; then
    return 0
  fi
  curl -fsSL https://install.determinate.systems/nix \
    | sh -s -- install linux --no-confirm >/dev/null
}

nix_group() {
  if getent group nix-users >/dev/null 2>&1; then
    printf 'nix-users\n'
  elif getent group nix >/dev/null 2>&1; then
    printf 'nix\n'
  else
    groupadd nix-users
    printf 'nix-users\n'
  fi
}

authorize_tog_for_nix() {
  local group
  group="$(nix_group)"
  usermod -aG "$group" "$TOG_USER"
}

install_direnv_and_nix_direnv() {
  apt_get_install direnv
  if [[ ! -d "${TOG_HOME}/.nix-direnv" ]]; then
    sudo -u "$TOG_USER" git clone --depth 1 \
      https://github.com/nix-community/nix-direnv "${TOG_HOME}/.nix-direnv"
  fi
  mkdir -p "${TOG_HOME}/.config/direnv"
  cat > "${TOG_HOME}/.config/direnv/direnvrc" <<'EOF'
source $HOME/.nix-direnv/direnvrc
EOF
}

configure_shell() {
  local bashrc="${TOG_HOME}/.bashrc"
  touch "$bashrc"
  if ! grep -q 'profile.d/nix.sh' "$bashrc"; then
    cat >> "$bashrc" <<'EOF'

# nix (multi-user install)
if [ -e /etc/profile.d/nix.sh ]; then
  . /etc/profile.d/nix.sh
fi
EOF
  fi
  if ! grep -q 'direnv hook bash' "$bashrc"; then
    cat >> "$bashrc" <<'EOF'

# direnv
eval "$(direnv hook bash)"
EOF
  fi
}

checkout_repository() {
  local parent
  parent="$(dirname "$TOG_CHECKOUT_DIR")"
  sudo -u "$TOG_USER" mkdir -p "$parent"
  chown "$TOG_USER:$TOG_USER" "$parent"
  if [[ ! -d "$TOG_CHECKOUT_DIR/.git" ]]; then
    sudo -u "$TOG_USER" git clone "$TOG_REPO_URL" "$TOG_CHECKOUT_DIR"
  fi
  sudo -u "$TOG_USER" direnv allow "$TOG_CHECKOUT_DIR"
}

finalize() {
  chown -R "$TOG_USER:$TOG_USER" "$TOG_HOME"
  touch "$SETUP_MARKER"
}

install_apt_packages
create_tog_user
install_nix
authorize_tog_for_nix
install_direnv_and_nix_direnv
configure_shell
checkout_repository
finalize
printf 'host setup complete for user %s\n' "$TOG_USER"
