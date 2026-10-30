#!/usr/bin/env bash
# Valide la structure d'un dépôt pacman généré par build_repository.sh.
#
# Usage:
#   packaging/arch/validate_repository.sh DIR

set -euo pipefail

dir="${1:-}"
if [[ -z "${dir}" || ! -d "${dir}" ]]; then
    echo "Usage: $0 DIR" >&2
    exit 2
fi

for arch in x86_64 aarch64; do
    arch_dir="${dir}/${arch}"
    if [[ ! -d "${arch_dir}" ]]; then
        echo "Missing arch directory: ${arch_dir}" >&2
        exit 1
    fi
    if [[ ! -f "${arch_dir}/ltbox.db.tar.gz" ]]; then
        echo "Missing package database: ${arch_dir}/ltbox.db.tar.gz" >&2
        exit 1
    fi
    if [[ ! -f "${arch_dir}/ltbox.db.tar.gz.sig" ]]; then
        echo "Missing database signature: ${arch_dir}/ltbox.db.tar.gz.sig" >&2
        exit 1
    fi
    packages=( "${arch_dir}"/ltbox-*-1-${arch}.pkg.tar.zst )
    if [[ ${#packages[@]} -eq 0 ]]; then
        echo "No packages found for ${arch}" >&2
        exit 1
    fi
    for package in "${packages[@]}"; do
        if [[ ! -f "${package}.sha256" ]]; then
            echo "Missing package checksum: ${package}.sha256" >&2
            exit 1
        fi
        (cd "${arch_dir}" && sha256sum --check "$(basename "${package}.sha256")")
    done
done

if [[ ! -f "${dir}/ltbox-repo.asc" ]]; then
    echo "Missing repository public key: ${dir}/ltbox-repo.asc" >&2
    exit 1
fi

if ! gpg --batch --quiet --verify "${dir}/x86_64/ltbox.db.tar.gz.sig" "${dir}/x86_64/ltbox.db.tar.gz" >/dev/null 2>&1; then
    echo "Repository database signature is invalid." >&2
    exit 1
fi

if [[ ! -f "${dir}/index.html" || ! -f "${dir}/.nojekyll" ]]; then
    echo "Missing GitHub Pages index.html or .nojekyll." >&2
    exit 1
fi

echo "Arch repository ${dir} is valid."
