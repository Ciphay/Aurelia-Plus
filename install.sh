#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")" && pwd)
bin_dir=${AURELIA_BIN_DIR:-$HOME/.local/bin}
gns_work=${GNS_WORK:-$root/plus/run/gns}
export GNS_WORK=$gns_work
GNS_URL=https://github.com/ValveSoftware/GameNetworkingSockets
GNS_COMMIT=d534c19aa760df3fb75fd20db13ba1932b8a5463
PROTOBUF_URL=https://github.com/protocolbuffers/protobuf
PROTOBUF_TAG=v36.1

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merro: %s\033[0m\n' "$*" >&2; exit 1; }
[[ $(uname -s) == Linux ]] || die "only Linux is supported"
[[ $(uname -m) == x86_64 ]] || die "only x86_64 is supported"
[[ $EUID -ne 0 ]] || die "do not run as root: the script asks for sudo only to install packages"

need_cmds=(gcc g++ make git curl python3 cmake ninja pkg-config x86_64-w64-mingw32-gcc x86_64-w64-mingw32-g++ x86_64-w64-mingw32-windres i686-w64-mingw32-gcc i686-w64-mingw32-g++)
missing_sys() {
  local c
  for c in "${need_cmds[@]}"; do command -v "$c" > /dev/null || return 0; done
  pkg-config --exists openssl || return 0
  [[ -f /usr/x86_64-w64-mingw32/lib/libpthread.a && -f /usr/i686-w64-mingw32/lib/libpthread.a ]] || return 0
  return 1
}
if missing_sys; then
  step "Installing system dependencies (needs sudo)"
  command -v sudo > /dev/null || die "sudo not found; install the dependencies manually: ${need_cmds[*]} and the OpenSSL development libraries"
  . /etc/os-release
  case " ${ID:-} ${ID_LIKE:-} " in
    *" arch "* | *" cachyos "*)
      sudo pacman -S --needed --noconfirm base-devel git curl python cmake ninja pkgconf openssl mingw-w64-gcc mingw-w64-winpthreads ;;
    *" debian "* | *" ubuntu "*)
      sudo apt-get update
      sudo apt-get install -y build-essential git curl python3 cmake ninja-build pkg-config libssl-dev \
        gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 gcc-mingw-w64-i686 g++-mingw-w64-i686 mingw-w64-tools binutils-mingw-w64
      for t in x86_64 i686; do
        for tool in gcc g++; do
          [[ -e /usr/bin/$t-w64-mingw32-$tool-posix ]] && sudo update-alternatives --set "$t-w64-mingw32-$tool" "/usr/bin/$t-w64-mingw32-$tool-posix"
        done
      done ;;
    *" fedora "* | *" rhel "*)
      sudo dnf install -y gcc gcc-c++ make git curl python3 cmake ninja-build pkgconf-pkg-config openssl-devel \
        mingw64-gcc-c++ mingw32-gcc-c++ mingw64-winpthreads-static mingw32-winpthreads-static mingw64-binutils mingw32-binutils ;;
    *) die "unrecognized distribution (${PRETTY_NAME:-?}); install: ${need_cmds[*]} and the OpenSSL development libraries, then run again" ;;
  esac
  missing_sys && die "tools are still missing after the installation; check the output above"
fi

if ! command -v cargo > /dev/null; then
  [[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
fi
if ! command -v cargo > /dev/null; then
  step "Installing Rust (rustup)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
fi

mkdir -p "$gns_work"
fetch_git() {
  local url=$1 dir=$2 ref=$3
  if [[ -d "$dir/.git" && $(cat "$dir/.pinned-ref" 2> /dev/null) == "$ref" ]]; then return 0; fi
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch -q --depth 1 origin "$ref"
  git -C "$dir" checkout -q FETCH_HEAD
  printf '%s' "$ref" > "$dir/.pinned-ref"
}
step "Downloading GameNetworkingSockets ($GNS_COMMIT) and protobuf ($PROTOBUF_TAG)"
fetch_git "$GNS_URL" "$gns_work/gns" "$GNS_COMMIT"
fetch_git "$PROTOBUF_URL" "$gns_work/protobuf" "$PROTOBUF_TAG"

step "Building the host protoc ($PROTOBUF_TAG)"
bash "$root/plus/tools/gns/build-protoc-host.sh"
export PROTOC=$gns_work/host-protoc/protoc
for arch in x86_64 i686; do
  if [[ ! -f "$gns_work/build-gns-$arch/src/libGameNetworkingSockets_s.a" ]]; then
    step "Building protobuf and GameNetworkingSockets ($arch)"
    bash "$root/plus/tools/gns/build-protobuf.sh" "$arch"
    bash "$root/plus/tools/gns/build-gns.sh" "$arch"
  fi
done

step "Building the steamclient shim (with GameNetworkingSockets)"
bash "$root/plus/prepare.sh"
dll=$root/plus/run/shimsteam/legacycompat/steamclient64.dll
[[ -f $dll ]] || die "the shim was not built"
[[ $(stat -c %s "$dll") -gt 4000000 ]] || die "the shim was built without GameNetworkingSockets (steamclient64.dll is $(stat -c %s "$dll") bytes)"

step "Building Aurelia"
( cd "$root/aurelia" && cargo build --release )
built=$root/aurelia/target/release/aurelia
[[ -x $built ]] || die "the binary was not built"
"$built" play --help 2>&1 | grep -q -- '--plus' || die "the binary was built without --plus support (the shim was not embedded)"

step "Installing the binary in $bin_dir"
mkdir -p "$bin_dir"
cp "$built" "$bin_dir/aurelia.new"
mv -f "$bin_dir/aurelia.new" "$bin_dir/aurelia"
path_changed=
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    case "$(basename "${SHELL:-bash}")" in
      fish) rc=$HOME/.config/fish/conf.d/aurelia.fish; line="fish_add_path -g \"$bin_dir\"" ;;
      zsh) rc=${ZDOTDIR:-$HOME}/.zshenv; line="export PATH=\"$bin_dir:\$PATH\"" ;;
      *) rc=$HOME/.bashrc; line="export PATH=\"$bin_dir:\$PATH\"" ;;
    esac
    mkdir -p "$(dirname "$rc")"
    grep -qsF "$line" "$rc" || printf '\n%s\n' "$line" >> "$rc"
    path_changed=1 ;;
esac
aurelia=$bin_dir/aurelia

if [[ -z ${AURELIA_INSTALL_SKIP_SERVICE:-} ]]; then
  step "Installing the daemon service (systemd --user)"
  "$aurelia" service install
fi
if [[ -z ${AURELIA_INSTALL_SKIP_LOGIN:-} ]]; then
  if ! "$aurelia" login --health 2> /dev/null | grep -q '^Session *: authenticated'; then
    step "Steam login (the official Proton and your games need your account)"
    [[ -t 0 ]] || die "no terminal available for the login; run '$aurelia login' and then ./install.sh again"
    "$aurelia" login
  fi
  if ! "$aurelia" proton list 2> /dev/null | grep -q '^Valve *Proton - Experimental .*installed'; then
    step "Installing Proton - Experimental"
    "$aurelia" proton install "Proton - Experimental"
  fi
fi

step "Done"
[[ -z $path_changed ]] || echo "Open a new terminal (or run: export PATH=\"$bin_dir:\$PATH\") so the 'aurelia' command works."
echo "To play:  aurelia play <game id> --plus      (list your games: aurelia list)"
