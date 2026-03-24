#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "error: setup_linux_cross_build_env.sh only supports Linux runners" >&2
  exit 1
fi

zig_version="${ZIG_VERSION:-0.15.2}"
zig_archive="zig-x86_64-linux-${zig_version}.tar.xz"
runner_temp="${RUNNER_TEMP:-/tmp}"
zig_install_root="${runner_temp}/zig-${zig_version}"
zig_unpack_dir="${zig_install_root}/zig-x86_64-linux-${zig_version}"
enable_zig_setup="${IMAGO_ENABLE_ZIG_SETUP:-false}"
expected_target="${IMAGO_EXPECTED_TARGET:-}"
cargo_zigbuild_version="${CARGO_ZIGBUILD_VERSION:-0.22.1}"

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  build-essential \
  ca-certificates \
  clang \
  cmake \
  g++-aarch64-linux-gnu \
  g++-arm-linux-gnueabihf \
  g++-riscv64-linux-gnu \
  gcc-aarch64-linux-gnu \
  gcc-arm-linux-gnueabihf \
  gcc-riscv64-linux-gnu \
  libc6-dev-arm64-cross \
  libc6-dev-armhf-cross \
  libc6-dev-riscv64-cross \
  libclang-dev \
  libssl-dev \
  wget \
  xz-utils \
  binutils-aarch64-linux-gnu \
  binutils-arm-linux-gnueabihf \
  binutils-riscv64-linux-gnu
sudo rm -rf /var/lib/apt/lists/*

if [[ "${enable_zig_setup}" == "true" ]]; then
  mkdir -p "${zig_install_root}"
  if [[ ! -x "${zig_unpack_dir}/zig" ]]; then
    curl -fsSL -o "${runner_temp}/${zig_archive}" "https://ziglang.org/download/${zig_version}/${zig_archive}"
    tar -xJf "${runner_temp}/${zig_archive}" -C "${zig_install_root}"
    rm -f "${runner_temp:?}/${zig_archive}"
  fi

  export PATH="${zig_unpack_dir}:${PATH}"
  if [[ -n "${GITHUB_PATH:-}" ]]; then
    echo "${zig_unpack_dir}" >> "${GITHUB_PATH}"
  fi

  current_cargo_zigbuild_version=""
  if command -v cargo-zigbuild >/dev/null 2>&1; then
    current_cargo_zigbuild_version="$(cargo-zigbuild --version | awk '{print $2}')"
  fi

  if [[ "${current_cargo_zigbuild_version}" != "${cargo_zigbuild_version}" ]]; then
    cargo install --locked cargo-zigbuild --version "${cargo_zigbuild_version}" --force
  fi
fi

if [[ -n "${GITHUB_ENV:-}" ]]; then
  {
    echo "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc"
    echo "CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc"
    echo "CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc"
  } >> "${GITHUB_ENV}"
else
  export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
  export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc
  export CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
fi

command -v cargo >/dev/null
command -v aarch64-linux-gnu-gcc >/dev/null
command -v arm-linux-gnueabihf-gcc >/dev/null
command -v riscv64-linux-gnu-gcc >/dev/null

if [[ -n "${expected_target}" ]]; then
  rustup target list --installed | grep -qx "${expected_target}"
fi

if [[ "${enable_zig_setup}" == "true" ]]; then
  command -v zig >/dev/null
  cargo zigbuild --help >/dev/null
fi
