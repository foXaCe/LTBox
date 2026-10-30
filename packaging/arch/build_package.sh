#!/usr/bin/env bash
# Build an Arch Linux package (.pkg.tar.zst) from a published LTBox release.
#
# Usage:
#   packaging/arch/build_package.sh --tag vX.Y.Z --arch x86_64|aarch64 --out DIR
#
# On an Arch host it runs makepkg directly. Elsewhere it builds inside the
# official archlinux container so the resulting package is a genuine Arch
# package, not a repack on a foreign distro.

set -euo pipefail

tag=""
arch=""
out=""

usage() {
    echo "Usage: $0 --tag vX.Y.Z --arch x86_64|aarch64 --out DIR" >&2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag) tag="$2"; shift 2 ;;
        --arch) arch="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        *) usage; exit 2 ;;
    esac
done

if [[ -z "${tag}" || -z "${arch}" || -z "${out}" ]]; then
    usage
    exit 2
fi

if [[ ! "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
    echo "Release tag is not v-prefixed SemVer: ${tag}" >&2
    exit 2
fi

case "${arch}" in
    x86_64) asset="LTBox-linux_x86_64-${tag}.tar.gz" ;;
    aarch64) asset="LTBox-linux_arm64-${tag}.tar.gz" ;;
    *) echo "Unsupported arch: ${arch}" >&2; exit 2 ;;
esac

version="${tag#v}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
template="${repo_root}/arch/PKGBUILD.tmpl"
install_script="${repo_root}/arch/ltbox.install"

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

srcdir="${work}/src"
mkdir -p "${srcdir}"
cd "${srcdir}"

# --- Télécharge le binaire publié et son côté de somme de contrôle ---
if [[ -n "${GITHUB_TOKEN:-}" || -n "${GH_TOKEN:-}" ]]; then
    gh release download "${tag}" \
        --repo "${GITHUB_REPOSITORY:-miner7222/LTBox}" \
        --dir "${srcdir}" \
        --pattern "${asset}" \
        --pattern "${asset}.sha256" >/dev/null 2>&1 || {
        echo "gh release download failed; falling back to direct download" >&2
        curl -fsSL "https://github.com/miner7222/LTBox/releases/download/${tag}/${asset}" \
            -o "${asset}"
        curl -fsSL "https://github.com/miner7222/LTBox/releases/download/${tag}/${asset}.sha256" \
            -o "${asset}.sha256"
    }
else
    curl -fsSL "https://github.com/miner7222/LTBox/releases/download/${tag}/${asset}" \
        -o "${asset}"
    curl -fsSL "https://github.com/miner7222/LTBox/releases/download/${tag}/${asset}.sha256" \
        -o "${asset}.sha256"
fi
(cd "${srcdir}" && sha256sum --check "${asset}.sha256")

# --- Règles udev (source séparée du PKGBUILD) ---
curl -fsSL "https://raw.githubusercontent.com/miner7222/LTBox/${tag}/misc/udev/51-ltbox-qcom.rules" \
    -o "${srcdir}/51-ltbox-qcom.rules"

# --- Sommes de contrôle (une par arch ; l'autre reste inutilisée pour ce build) ---
asset_sha="$(sha256sum "${srcdir}/${asset}" | awk '{print $1}')"
udev_sha="$(sha256sum "${srcdir}/51-ltbox-qcom.rules" | awk '{print $1}')"
zero_sha="$(printf '0%.0s' {1..64})"
if [[ "${arch}" == "x86_64" ]]; then
    x86_64_sha="${asset_sha}"; aarch64_sha="${zero_sha}"
else
    x86_64_sha="${zero_sha}"; aarch64_sha="${asset_sha}"
fi

# --- Rend le PKGBUILD (les deux arches sont déclarées, makepkg choisit selon CARCH) ---
sed \
    -e "s/@@VERSION@@/${version}/g" \
    -e "s/@@SHA256_X86_64@@/${x86_64_sha}/g" \
    -e "s/@@SHA256_AARCH64@@/${aarch64_sha}/g" \
    -e "s/@@SHA256_UDEV@@/${udev_sha}/g" \
    "${template}" > "${srcdir}/PKGBUILD"
cp "${install_script}" "${srcdir}/ltbox.install"

out="$(realpath -m "${out}")"
mkdir -p "${out}"

# --- Construit le paquet ---
if command -v makepkg >/dev/null 2>&1; then
    # Hôte Arch : makepkg natif (pas de root nécessaire en pratique).
    (cd "${srcdir}" && makepkg --noconfirm --cleanbuild)
else
    # Hôte étranger : conteneur Arch officiel, makepkg en root (--asroot).
    docker run --rm \
        -v "${work}:/work:rw" \
        -w /work \
        archlinux:latest \
        bash -ceu 'cd /work/src && makepkg --asroot --noconfirm'
fi

cp "${srcdir}"/*.pkg.tar.zst "${out}/"
echo "Built Arch package(s):"
ls -l "${out}"/*.pkg.tar.zst
